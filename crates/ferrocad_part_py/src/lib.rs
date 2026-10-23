//! # ferrocad_part_py
//!
//! The **`Part`** workbench's Python module, built on
//! [`ferrocad_part`](https://crates.io/crates/ferrocad_part) (the object/property
//! types) and [`ferrocad_occt`](https://crates.io/crates/ferrocad_occt) (the kernel).
//!
//! This crate is the **composition root** for Part: it is the one place that links a
//! concrete kernel and calls [`ferrocad_part::register`], so Part itself stays
//! kernel-agnostic and `ferrocad_core` stays geometry-free (see
//! `docs/architecture/occt-integration.md` §4).
//!
//! It exposes two things to Python:
//!
//! - `Part.makeBox(l, w, h)` → a `Part.Shape`;
//! - `Part.Shape`, the opaque wrapper the document returns for `obj.Shape`.
//!
//! The second works through the converter hook in `ferrocad_py`: a `Property::Extension`
//! is opaque to core, so the owning module registers how to move it to and from Python.
//! Here that is `Part::PropertyPartShape` ↔ `Part.Shape`.
//!
//! # How it is meant to be loaded
//!
//! An application links this as an `rlib` and registers the `Part` module as a
//! built-in, so `import Part` shares one process-wide core with `import FreeCAD`:
//!
//! ```ignore
//! use ferrocad_part_py::Part as part_module;
//! pyo3::append_to_inittab!(part_module);
//! ```
//!
//! A separate `.so` cannot share that state (each extension module would statically
//! link its own `ferrocad_core`), which is why Part must live in the same image as
//! the core bindings.

#![allow(non_snake_case)]

mod containers;
mod curves;
mod link;
mod link_array;
mod solids;

use std::sync::{Arc, Mutex};

use ferrocad_core::{ExtensionData, ExtensionValue};
use ferrocad_geom::{ElementKind, ElementRef, Shape, TessellationOptions};
use ferrocad_occt::OcctBackend;
use ferrocad_part::{ShapeListProperty, ShapeProperty, SHAPE_LIST_TYPE_NAME, TYPE_NAME};
use pyo3::exceptions::{PyIOError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList, PyModule, PyTuple};
use pyo3::IntoPyObjectExt;

/// Register the Part kernel (OCCT) and its core types. Idempotent; every entry
/// point that can build a shape calls it first.
fn register_kernel() {
    ferrocad_part::register(Arc::new(OcctBackend::new()));
    link::register();
    link_array::register();
}

/// `Part.Shape`: an opaque handle to a kernel shape.
///
/// The `Mutex` is here at the Python boundary only: `Shape` is `Send` but not `Sync`
/// (see `ferrocad_geom`), and a `#[pyclass]` must be `Send + Sync`. It is not a lock
/// in the seam.
#[pyclass(name = "Shape", module = "Part", subclass)]
pub struct PartShape {
    shape: Mutex<Option<Shape>>,
}

impl PartShape {
    /// The null shape.
    pub(crate) fn empty() -> Self {
        PartShape {
            shape: Mutex::new(None),
        }
    }

    /// A second handle to the same shape (for `Shape.copy` and subclasses).
    pub(crate) fn copy_handle(&self) -> Self {
        PartShape {
            shape: Mutex::new(self.shape()),
        }
    }

    /// A copy of the wrapped shape (shape handles are cheap to clone).
    pub(crate) fn shape(&self) -> Option<Shape> {
        self.shape.lock().unwrap().clone()
    }

    /// Wrap an existing kernel shape.
    pub(crate) fn from_shape(shape: Shape) -> Self {
        PartShape {
            shape: Mutex::new(Some(shape)),
        }
    }

    /// Replace the wrapped shape (a subclass rebuilds its base shape in place).
    pub(crate) fn replace_shape(&self, shape: Option<Shape>) {
        *self.shape.lock().unwrap_or_else(|e| e.into_inner()) = shape;
    }

    /// The shape's topological kind (`Shape.ShapeType`), inferred from its
    /// sub-shape lists.
    ///
    /// The seam has no "what kind of shape is this" query, so this walks the
    /// element lists from the highest dimension down and reports the first kind
    /// that is present:
    ///
    /// - exactly one element of that kind → that kind's name (`Solid`, `Shell`,
    ///   `Face`, `Wire`, `Edge`, `Vertex`);
    /// - more than one element of that kind → `Compound` (a compound of two
    ///   solids, two faces, …);
    /// - no elements of any kind → `Compound` (this also covers the null shape).
    ///
    /// A box therefore reports `Solid` (its one solid wins over its six faces),
    /// while a single edge reports `Edge`. The inference cannot tell a compound
    /// that happens to hold exactly one solid from that solid itself, so the
    /// former is reported as `Solid` — upstream would say `Compound`.
    fn shape_type(&self) -> &'static str {
        let Some(shape) = self.shape() else {
            return "Compound";
        };
        let backend = ferrocad_part::backend();
        for (kind, name) in [
            (ElementKind::Solid, "Solid"),
            (ElementKind::Shell, "Shell"),
            (ElementKind::Face, "Face"),
            (ElementKind::Wire, "Wire"),
            (ElementKind::Edge, "Edge"),
            (ElementKind::Vertex, "Vertex"),
        ] {
            match backend.elements(&shape, kind).len() {
                0 => continue,
                1 => return name,
                _ => return "Compound",
            }
        }
        "Compound"
    }

    /// The sub-shapes of one kind, in the kernel's order.
    pub(crate) fn sub_shapes(&self, kind: ElementKind) -> PyResult<Vec<Shape>> {
        let shape = self
            .shape()
            .ok_or_else(|| PyValueError::new_err("a null shape has no sub-shapes"))?;
        Ok(ferrocad_part::backend()
            .elements(&shape, kind)
            .into_iter()
            .map(|element| element.shape)
            .collect())
    }

    /// Where `target` sits among this shape's sub-elements: its kind name and
    /// 1-based index, matching upstream's `findSubShape` tuple.
    fn locate(&self, target: &Shape) -> Option<(&'static str, usize)> {
        let shape = self.shape()?;
        let backend = ferrocad_part::backend();
        for (kind, name) in ELEMENT_KINDS {
            for (index, element) in backend.elements(&shape, kind).iter().enumerate() {
                if backend.same_shape(&element.shape, target) {
                    return Some((name, index + 1));
                }
            }
        }
        None
    }

    /// The topological kind of `shape`, from its sub-shape lists (the same walk
    /// as `shape_type`).
    fn kind_of(shape: &Shape) -> Option<ElementKind> {
        let backend = ferrocad_part::backend();
        ELEMENT_KINDS
            .iter()
            .map(|(kind, _)| *kind)
            .find(|kind| backend.elements(shape, *kind).len() == 1)
    }
}

#[pymethods]
impl PartShape {
    /// `Part.Shape()` is the null shape; `Part.Shape(other)` wraps a copy of
    /// `other` (upstream's copy constructor).
    #[new]
    #[pyo3(signature = (value = None))]
    fn new(value: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        match value {
            None => Ok(Self::empty()),
            Some(value) => Ok(Self::from_shape(shape_argument(value)?)),
        }
    }

    /// A copy of the shape (`Shape.copy`); the kernel handle is reference-counted
    /// so no geometry is duplicated.
    #[pyo3(signature = (noElementMap = false))]
    fn copy(&self, noElementMap: bool) -> Self {
        let _ = noElementMap;
        self.copy_handle()
    }

    /// The shape's placement. Kernel shapes are stored with baked geometry, so
    /// this is always the identity (`Shape.transformed` bakes the matrix in).
    #[getter]
    fn Placement(&self, py: Python<'_>) -> PyResult<PyObject> {
        let module = py.import("ferrocad")?;
        Ok(module.getattr("Placement")?.call0()?.unbind())
    }

    /// Assigning a placement bakes it into the geometry (there is no separate
    /// shape location in this kernel).
    #[setter]
    fn set_Placement(&self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let matrix = matrix_from_py(&value.call_method0("toMatrix")?)?;
        let shape = self.named_shape()?;
        let moved = ferrocad_part::backend()
            .transform(&shape, &matrix)
            .map_err(kernel_error)?;
        self.replace_shape(Some(moved));
        Ok(())
    }

    /// A copy translated by `vector` (`Shape.translated`).
    fn translated(&self, vector: &Bound<'_, PyAny>) -> PyResult<PartShape> {
        let offset = curves::vector(vector)?;
        self.transformed_by(&translation_matrix(offset))
    }

    /// A copy scaled uniformly by `factor` about `center` (`Shape.scaled`).
    #[pyo3(signature = (factor, center = None))]
    fn scaled(
        &self,
        factor: f64,
        center: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PartShape> {
        let center = center
            .map(curves::vector)
            .transpose()?
            .unwrap_or_else(ferrocad_curves::Vector3::zero);
        self.transformed_by(&scale_matrix(factor, center))
    }

    /// Mirror this shape across the plane through `point` with `normal`, in place
    /// (`Shape.mirror`).
    fn mirror(
        slf: &Bound<'_, Self>,
        point: &Bound<'_, PyAny>,
        normal: &Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let point = curves::vector(point)?;
        let normal = curves::vector(normal)?;
        let matrix = mirror_matrix(point, normal)?;
        let shape = slf.borrow().named_shape()?;
        let moved = ferrocad_part::backend()
            .transform(&shape, &matrix)
            .map_err(kernel_error)?;
        slf.borrow_mut().replace_shape(Some(moved));
        Ok(slf.clone().into_any().unbind())
    }

    /// The direct children (`Shape.childShapes`): the highest-level sub-shapes.
    #[pyo3(signature = (shape_type = None))]
    fn childShapes(
        &self,
        py: Python<'_>,
        shape_type: Option<&str>,
    ) -> PyResult<Vec<Py<PartShape>>> {
        let _ = shape_type;
        for kind in [
            ElementKind::Solid,
            ElementKind::Shell,
            ElementKind::Face,
            ElementKind::Wire,
            ElementKind::Edge,
            ElementKind::Vertex,
        ] {
            let shapes = self.sub_shapes(kind)?;
            if !shapes.is_empty() {
                return shapes
                    .into_iter()
                    .map(|shape| Py::new(py, PartShape::from_shape(shape)))
                    .collect();
            }
        }
        Ok(Vec::new())
    }

    /// A transformed copy (`Shape.transformed(matrix, copy=True)`).
    #[pyo3(signature = (matrix, copy = true))]
    fn transformed(
        &self,
        matrix: &Bound<'_, PyAny>,
        copy: bool,
    ) -> PyResult<PartShape> {
        let _ = copy;
        let shape = self.named_shape()?;
        let matrix = matrix_from_py(matrix)?;
        let moved = ferrocad_part::backend()
            .transform(&shape, &matrix)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(moved))
    }

    /// Transform this shape in place and return it (`Shape.transformShape`).
    #[pyo3(signature = (matrix, copy = true))]
    fn transformShape<'py>(
        slf: &Bound<'py, Self>,
        matrix: &Bound<'_, PyAny>,
        copy: bool,
    ) -> PyResult<PyObject> {
        let _ = copy;
        let shape = slf.borrow().named_shape()?;
        let matrix = matrix_from_py(matrix)?;
        let moved = ferrocad_part::backend()
            .transform(&shape, &matrix)
            .map_err(kernel_error)?;
        slf.borrow_mut().replace_shape(Some(moved));
        Ok(slf.clone().into_any().unbind())
    }

    /// A copy with redundant edges/faces unified (`Shape.cleaned`).
    fn cleaned(&self) -> Self {
        let Some(shape) = self.shape() else {
            return Self::empty();
        };
        match ferrocad_part::backend().clean(&shape) {
            Some(cleaned) => Self::from_shape(cleaned),
            None => self.copy_handle(),
        }
    }

    /// Whether this is the empty/null shape (FreeCAD's `Shape.isNull()`).
    fn isNull(&self) -> bool {
        self.shape.lock().unwrap().is_none()
    }

    /// Whether the kernel finds every sub-shape valid (`Shape.isValid`).
    ///
    /// A null shape, or a kernel that cannot run the check, reports `false`.
    fn isValid(&self) -> bool {
        match self.shape() {
            Some(shape) => ferrocad_part::backend().is_valid(&shape),
            None => false,
        }
    }

    /// Whether two handles are the same kernel object.
    fn isSame(&self, other: &Bound<'_, PyAny>) -> bool {
        let Ok(other) = other.downcast::<PartShape>() else {
            return false;
        };
        // Clone each shape out (releasing each lock at statement end) before
        // comparing, so `shape.isSame(shape)` cannot deadlock on one `Mutex`.
        let a = self.shape();
        let b = other.borrow().shape();
        match (a, b) {
            (Some(x), Some(y)) => ferrocad_part::backend().same_shape(&x, &y),
            _ => false,
        }
    }

    /// Write the shape as BREP to `filename` (FreeCAD's `Shape.exportBrep`).
    fn exportBrep(&self, filename: &str) -> PyResult<()> {
        let shape = self
            .shape()
            .ok_or_else(|| PyValueError::new_err("cannot export a null shape"))?;
        let bytes = ferrocad_part::backend().save_shape(&shape);
        std::fs::write(filename, bytes).map_err(|e| PyIOError::new_err(e.to_string()))
    }

    /// The BREP encoding of the shape, as `bytes`.
    fn exportBrepToString(&self, py: Python<'_>) -> PyResult<Py<PyBytes>> {
        let shape = self
            .shape()
            .ok_or_else(|| PyValueError::new_err("cannot export a null shape"))?;
        let bytes = ferrocad_part::backend().save_shape(&shape);
        Ok(PyBytes::new(py, &bytes).unbind())
    }

    /// A display mesh with per-primitive sub-element ids and names.
    ///
    /// A FerroCAD extension (upstream `Shape.tessellate` returns bare vertices and
    /// facets): the dict carries the arrays of a `ferrocad_geom::Tessellation`, so a
    /// host can draw the shape **and** map a pick back to `Face6`/`Edge3`.
    ///
    /// `deflection` is the linear deflection in model units; `angular` is the angular
    /// deflection in radians.
    #[pyo3(signature = (deflection = 0.5, angular = 0.5))]
    fn mesh(&self, py: Python<'_>, deflection: f64, angular: f64) -> PyResult<Py<PyAny>> {
        let shape = self
            .shape()
            .ok_or_else(|| PyValueError::new_err("cannot tessellate a null shape"))?;
        let options = TessellationOptions {
            linear_deflection: Some(deflection),
            angular_deflection: angular,
            relative: false,
        };
        let mesh = ferrocad_part::backend()
            .tessellate(&shape, &options)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;

        let dict = PyDict::new(py);
        dict.set_item(
            "positions",
            PyList::new(py, mesh.positions.iter().map(|p| (p[0], p[1], p[2])))?,
        )?;
        dict.set_item("triangles", mesh.triangles)?;
        dict.set_item("triangle_parts", mesh.triangle_parts)?;
        dict.set_item("edges", mesh.edges)?;
        dict.set_item("segment_parts", mesh.segment_parts)?;
        dict.set_item("vertices", mesh.vertices)?;
        dict.set_item("vertex_parts", mesh.vertex_parts)?;
        dict.set_item("parts", mesh.parts)?;
        Ok(dict.into_any().unbind())
    }

    fn __repr__(&self) -> String {
        match self.shape.lock().unwrap().as_ref() {
            Some(shape) => format!("Part.Shape({shape:?})"),
            None => "Part.Shape()".to_string(),
        }
    }

    /// The edges (`Shape.Edges`) as `Part.Edge` wrappers.
    #[getter]
    fn Edges(&self, py: Python<'_>) -> PyResult<Vec<Py<curves::PyEdge>>> {
        self.sub_shapes(ElementKind::Edge)?
            .into_iter()
            .map(|shape| {
                Py::new(
                    py,
                    curves::PyEdge::from_entry(curves::EdgeEntry {
                        shape,
                        curve: None,
                    }),
                )
            })
            .collect()
    }

    /// The wires (`Shape.Wires`) as `Part.Shape` handles.
    #[getter]
    fn Wires(&self, py: Python<'_>) -> PyResult<Vec<Py<PartShape>>> {
        self.sub_shapes(ElementKind::Wire)?
            .into_iter()
            .map(|shape| Py::new(py, PartShape::from_shape(shape)))
            .collect()
    }

    /// The faces (`Shape.Faces`) as `Part.Shape` handles.
    #[getter]
    fn Faces(&self, py: Python<'_>) -> PyResult<Vec<Py<PartShape>>> {
        self.sub_shapes(ElementKind::Face)?
            .into_iter()
            .map(|shape| Py::new(py, PartShape::from_shape(shape)))
            .collect()
    }

    /// The vertices (`Shape.Vertexes`) as `Part.Vertex` wrappers with `Point`.
    #[getter]
    fn Vertexes(&self, py: Python<'_>) -> PyResult<Vec<Py<curves::PyVertex>>> {
        self.sub_shapes(ElementKind::Vertex)?
            .into_iter()
            .map(|shape| Py::new(py, curves::PyVertex::from_shape(shape)?))
            .collect()
    }

    /// The direct sub-shapes (`Shape.SubShapes`): the same list
    /// [`childShapes`](Self::childShapes) returns, matching upstream's one
    /// immediate solid for a box.
    #[getter]
    fn SubShapes(&self, py: Python<'_>) -> PyResult<Vec<Py<PartShape>>> {
        self.childShapes(py, None)
    }

    /// The solids (`Shape.Solids`) as `Part.Shape` handles.
    #[getter]
    fn Solids(&self, py: Python<'_>) -> PyResult<Vec<Py<PartShape>>> {
        self.sub_shapes(ElementKind::Solid)?
            .into_iter()
            .map(|shape| Py::new(py, PartShape::from_shape(shape)))
            .collect()
    }

    /// The shells (`Shape.Shells`) as `Part.Shape` handles.
    #[getter]
    fn Shells(&self, py: Python<'_>) -> PyResult<Vec<Py<PartShape>>> {
        self.sub_shapes(ElementKind::Shell)?
            .into_iter()
            .map(|shape| Py::new(py, PartShape::from_shape(shape)))
            .collect()
    }

    /// The shape's topological kind (`Shape.ShapeType`); see the `shape_type`
    /// inference above.
    #[getter]
    fn ShapeType(&self) -> &'static str {
        self.shape_type()
    }

    /// The backing surface (`Face.Surface`), when the shape is a face.
    #[getter]
    fn Surface(&self) -> Option<curves::PySurface> {
        let shape = self.shape()?;
        ferrocad_part::backend()
            .surface_type(&shape)
            .map(curves::PySurface::new)
    }

    /// The outer boundary of a face (`Face.OuterWire`).
    #[getter]
    fn OuterWire(&self, py: Python<'_>) -> PyResult<Option<Py<curves::PyWire>>> {
        let Some(shape) = self.shape() else {
            return Ok(None);
        };
        match ferrocad_part::backend().outer_wire(&shape) {
            Some(wire) => Ok(Some(Py::new(py, curves::PyWire::from_shape(wire))?)),
            None => Ok(None),
        }
    }

    /// The axis-aligned bounds as a `ferrocad.BoundBox` (`Shape.BoundBox`).
    #[getter]
    fn BoundBox(&self, py: Python<'_>) -> PyResult<PyObject> {
        let module = py.import("ferrocad")?;
        let bound_box = module.getattr("BoundBox")?;
        let Some(shape) = self.shape() else {
            return Ok(bound_box.call0()?.unbind());
        };
        match ferrocad_part::backend().bounds(&shape) {
            Some((min, max)) => Ok(bound_box
                .call1((min.x, min.y, min.z, max.x, max.y, max.z))?
                .unbind()),
            None => Ok(bound_box.call0()?.unbind()),
        }
    }

    /// The shape's length (`Shape.Length`).
    ///
    /// Upstream sums the lengths of the edges of every face, so a solid counts
    /// each edge once per adjacent face: a 2x1x2 box is 40, not 20. This kernel's
    /// linear properties count each edge once, so for a solid or a shell we add up
    /// the perimeter of each face instead. Edges, wires and faces keep the
    /// kernel's linear-properties value (a wire still counts each edge once).
    #[getter]
    fn Length(&self) -> f64 {
        match self.shape_type() {
            "Solid" | "Shell" => {
                let backend = ferrocad_part::backend();
                self.sub_shapes(ElementKind::Face)
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|face| backend.mass_properties(&face))
                    .map(|mass| mass.length)
                    .sum()
            }
            _ => self.mass_properties().map(|mass| mass.length).unwrap_or(0.0),
        }
    }

    /// The shape's mass (`Shape.Mass`). This kernel has unit density, so the mass
    /// is the volume.
    #[getter]
    fn Mass(&self) -> f64 {
        self.Volume()
    }

    /// The first moments of volume (`Shape.StaticMoments`), read from the kernel's
    /// own GProp integral so the precision matches upstream.
    #[getter]
    fn StaticMoments(&self) -> (f64, f64, f64) {
        if let Some(shape) = self.shape() {
            if let Some([x, y, z]) = ferrocad_part::backend().static_moments(&shape) {
                return (x, y, z);
            }
        }
        match self.mass_properties() {
            Some(mass) => (
                mass.center_of_mass.x * mass.volume,
                mass.center_of_mass.y * mass.volume,
                mass.center_of_mass.z * mass.volume,
            ),
            None => (0.0, 0.0, 0.0),
        }
    }

    /// The rotational inertia matrix (`Shape.MatrixOfInertia`), as a
    /// `ferrocad.Matrix` whose top-left 3x3 block is the inertia tensor.
    ///
    /// A null shape, or one the kernel cannot measure, reports the null matrix.
    #[getter]
    fn MatrixOfInertia(&self, py: Python<'_>) -> PyResult<PyObject> {
        let matrix = self
            .shape()
            .and_then(|shape| ferrocad_part::backend().inertia_matrix(&shape));
        let module = py.import("ferrocad")?;
        let matrix_type = module.getattr("Matrix")?;
        match matrix {
            Some(matrix) => {
                let values = PyTuple::new(py, matrix.m)?;
                Ok(matrix_type.call1(values)?.unbind())
            }
            None => Ok(matrix_type.call0()?.unbind()),
        }
    }

    /// The principal moments, axes and radii of gyration
    /// (`Shape.PrincipalProperties`), as upstream's dict.
    ///
    /// A null shape, or one the kernel cannot measure, reports an empty dict.
    #[getter]
    fn PrincipalProperties(&self, py: Python<'_>) -> PyResult<PyObject> {
        let dict = PyDict::new(py);
        let Some(properties) = self
            .shape()
            .and_then(|shape| ferrocad_part::backend().principal_properties(&shape))
        else {
            return Ok(dict.into_any().unbind());
        };
        dict.set_item("SymmetryAxis", properties.symmetry_axis)?;
        dict.set_item("SymmetryPoint", properties.symmetry_point)?;
        dict.set_item("Moments", properties.moments)?;
        dict.set_item(
            "FirstAxisOfInertia",
            curves::py_vector(py, properties.axes.0)?,
        )?;
        dict.set_item(
            "SecondAxisOfInertia",
            curves::py_vector(py, properties.axes.1)?,
        )?;
        dict.set_item(
            "ThirdAxisOfInertia",
            curves::py_vector(py, properties.axes.2)?,
        )?;
        dict.set_item("RadiusOfGyration", properties.radius_of_gyration)?;
        Ok(dict.into_any().unbind())
    }

    /// The surface area (`Shape.Area`).
    #[getter]
    fn Area(&self) -> f64 {
        self.mass_properties().map(|mass| mass.area).unwrap_or(0.0)
    }

    /// The enclosed volume (`Shape.Volume`).
    #[getter]
    fn Volume(&self) -> f64 {
        self.mass_properties().map(|mass| mass.volume).unwrap_or(0.0)
    }

    /// The centre of mass (`Shape.CenterOfMass`).
    #[getter]
    fn CenterOfMass(&self, py: Python<'_>) -> PyResult<PyObject> {
        let point = self
            .mass_properties()
            .map(|mass| mass.center_of_mass)
            .unwrap_or_else(ferrocad_curves::Vector3::zero);
        curves::py_vector(py, point)
    }

    /// Upstream's alias for `CenterOfMass`.
    #[getter]
    fn CenterOfGravity(&self, py: Python<'_>) -> PyResult<PyObject> {
        self.CenterOfMass(py)
    }

    /// Upstream's element-map version. This kernel emits no element maps, so it
    /// is empty and `ElementMapSize` is zero (upstream's own tests treat that as
    /// "skip the map assertions").
    #[getter]
    fn ElementMapVersion(&self) -> &'static str {
        ""
    }

    #[getter]
    fn ElementMapSize(&self) -> usize {
        0
    }

    #[getter]
    fn ElementReverseMap(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        Ok(PyDict::new(py).unbind())
    }

    /// The serialised element map (`Shape.Content`). This kernel emits no element
    /// maps, so it is empty; upstream prints the same `<ElementMap/>` for a fresh
    /// box.
    #[getter]
    fn Content(&self) -> &'static str {
        "<ElementMap/>\n"
    }

    /// The element map (`Shape.ElementMap`), empty without TNP.
    #[getter]
    fn ElementMap(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        Ok(PyDict::new(py).unbind())
    }

    /// Upstream's history hasher (`Shape.Hasher`); this kernel keeps none.
    #[getter]
    fn Hasher(&self, py: Python<'_>) -> PyObject {
        py.None()
    }

    /// The owning module (`Shape.Module`).
    #[getter]
    fn Module(&self) -> &'static str {
        "Part"
    }

    /// The shape orientation (`Shape.Orientation`).
    #[getter]
    fn Orientation(&self) -> &'static str {
        let Some(shape) = self.shape() else {
            return "Forward";
        };
        ferrocad_part::backend()
            .orientation(&shape)
            .unwrap_or("Forward")
    }

    /// Upstream's type id for a `TopoShape`.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::TopoShape"
    }

    /// The compound children (`Shape.Compounds`). The seam has no enumeration for
    /// compounds inside a shape, so this is always empty.
    #[getter]
    fn Compounds(&self, py: Python<'_>) -> Py<PyList> {
        PyList::empty(py).unbind()
    }

    /// The comp-solid children (`Shape.CompSolids`). The seam has no enumeration
    /// for comp-solids inside a shape, so this is always empty.
    #[getter]
    fn CompSolids(&self, py: Python<'_>) -> Py<PyList> {
        PyList::empty(py).unbind()
    }

    /// `Shape.Vertex1`, `Shape.Edge2`, `Shape.Face3`, … — the kernel's 1-based
    /// sub-shape lists, as upstream's `TopoShape` shortcuts.
    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
        let Some((kind, index)) = sub_shape_name(name) else {
            return Err(pyo3::exceptions::PyAttributeError::new_err(format!(
                "'Part.Shape' object has no attribute '{name}'"
            )));
        };
        let shapes = self.sub_shapes(kind)?;
        let shape = shapes.get(index).ok_or_else(|| {
            pyo3::exceptions::PyAttributeError::new_err(format!(
                "'Part.Shape' object has no attribute '{name}'"
            ))
        })?
        .clone();
        match kind {
            ElementKind::Vertex => Ok(Py::new(py, curves::PyVertex::from_shape(shape)?)?.into_any()),
            ElementKind::Edge => Ok(Py::new(
                py,
                curves::PyEdge::from_entry(curves::EdgeEntry {
                    shape,
                    curve: None,
                }),
            )?
            .into_any()),
            _ => Ok(Py::new(py, PartShape::from_shape(shape))?.into_any()),
        }
    }

    /// `Shape.fuse(other)` — the union of the two shapes.
    ///
    /// `glue`, `tolerance` and `refine` are accepted for upstream signature
    /// compatibility; this slice runs the plain boolean and produces no element
    /// map, so `noElementMap` changes nothing.
    #[pyo3(signature = (other, glue = false, tolerance = 0.0, refine = true, noElementMap = false))]
    fn fuse(
        &self,
        other: &Bound<'_, PyAny>,
        glue: bool,
        tolerance: f64,
        refine: bool,
        noElementMap: bool,
    ) -> PyResult<PartShape> {
        let _ = (glue, tolerance, refine, noElementMap);
        self.boolean(other, BooleanOp::Fuse)
    }

    /// `Shape.cut(other)` — `self` with `other` removed.
    #[pyo3(signature = (other, glue = false, tolerance = 0.0, refine = true, noElementMap = false))]
    fn cut(
        &self,
        other: &Bound<'_, PyAny>,
        glue: bool,
        tolerance: f64,
        refine: bool,
        noElementMap: bool,
    ) -> PyResult<PartShape> {
        let _ = (glue, tolerance, refine, noElementMap);
        self.boolean(other, BooleanOp::Cut)
    }

    /// `Shape.common(other)` — the intersection of the two shapes.
    #[pyo3(signature = (other, glue = false, tolerance = 0.0, refine = true, noElementMap = false))]
    fn common(
        &self,
        other: &Bound<'_, PyAny>,
        glue: bool,
        tolerance: f64,
        refine: bool,
        noElementMap: bool,
    ) -> PyResult<PartShape> {
        let _ = (glue, tolerance, refine, noElementMap);
        self.boolean(other, BooleanOp::Common)
    }

    /// `Shape.multiFuse(shapes)` — fuse this shape with each shape in `shapes`.
    #[pyo3(signature = (shapes, glue = false, tolerance = 0.0, refine = true, noElementMap = false))]
    fn multiFuse(
        &self,
        shapes: &Bound<'_, PyAny>,
        glue: bool,
        tolerance: f64,
        refine: bool,
        noElementMap: bool,
    ) -> PyResult<PartShape> {
        let _ = (glue, tolerance, refine, noElementMap);
        let mut shape = self.named_shape()?;
        for item in shapes.try_iter()? {
            let other = shape_argument(&item?)?;
            shape = ferrocad_part::backend()
                .fuse(&shape, &other)
                .map_err(kernel_error)?
                .shape;
        }
        Ok(PartShape::from_shape(shape))
    }

    /// `Shape.extrude(vector)` — sweep the shape along `vector`.
    fn extrude(&self, vector: &Bound<'_, PyAny>) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        let direction = curves::vector(vector)?;
        let extruded = ferrocad_part::backend()
            .extrude(&shape, direction)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(extruded))
    }

    /// `Shape.revolve(center, axis, angle=360)` — revolve the shape about the
    /// axis through `center` along `axis`.
    ///
    /// `angle` is in **degrees**, as upstream's `Shape.revolve`.
    #[pyo3(signature = (center, axis, angle = 360.0))]
    fn revolve(
        &self,
        center: &Bound<'_, PyAny>,
        axis: &Bound<'_, PyAny>,
        angle: f64,
    ) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        let center = curves::vector(center)?;
        let axis = curves::vector(axis)?;
        let revolved = ferrocad_part::backend()
            .revolve(&shape, center, axis, angle.to_radians())
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(revolved))
    }

    /// `Face.makeOffset(distance)` — the face grown (or shrunk) by `distance`
    /// in its plane. Upstream's deprecated alias for `makeOffset2D`.
    fn makeOffset(&self, distance: f64) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        let offset = ferrocad_part::backend()
            .make_offset(&shape, distance, 0)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(offset))
    }

    /// `Shape.makeOffset2D(offset, join=0, fill=False, openResult=False,
    /// intersection=False)` — planar offset of a face or wire.
    ///
    /// Only the unfilled form and face/wire inputs are implemented: `fill=True`
    /// (the ring between the source and the offset) is not, and neither are the
    /// `openResult`/`intersection` switches or the edge/compound inputs.
    #[pyo3(signature = (offset, *, join = 0, fill = false, openResult = false, intersection = false))]
    fn makeOffset2D(
        &self,
        offset: f64,
        join: i32,
        fill: bool,
        openResult: bool,
        intersection: bool,
    ) -> PyResult<PartShape> {
        let _ = (openResult, intersection);
        if fill {
            return Err(curves::OCCError::new_err(
                "makeOffset2D: filling the offset is not implemented yet",
            ));
        }
        let shape = self.named_shape()?;
        let offset = ferrocad_part::backend()
            .make_offset(&shape, offset, join)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(offset))
    }

    /// `Shape.makeThickness(faces, offset, tolerance, inter=False,
    /// self_inter=False, offset_mode=0, join=0)` — hollow the solid by `offset`,
    /// leaving `faces` open.
    ///
    /// The boolean switches and the offset mode are accepted for signature
    /// compatibility; the kernel always runs the plain skin offset.
    #[pyo3(signature = (faces, offset, tolerance, inter = false, self_inter = false, offset_mode = 0, join = 0))]
    fn makeThickness(
        &self,
        faces: &Bound<'_, PyAny>,
        offset: f64,
        tolerance: f64,
        inter: bool,
        self_inter: bool,
        offset_mode: i32,
        join: i32,
    ) -> PyResult<PartShape> {
        let _ = (inter, self_inter, offset_mode, join);
        let shape = self.named_shape()?;
        let faces = shape_sequence(faces)?;
        if faces.is_empty() {
            return Err(curves::OCCError::new_err(
                "makeThickness needs at least one face to leave open",
            ));
        }
        let thickened = ferrocad_part::backend()
            .make_thickness(&shape, &faces, offset, tolerance)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(thickened))
    }

    /// `Shape.removeSplitter()` — unify the redundant edges/faces the booleans
    /// leave behind (`ShapeUpgrade_UnifySameDomain`).
    fn removeSplitter(&self) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        match ferrocad_part::backend().clean(&shape) {
            Some(cleaned) => Ok(PartShape::from_shape(cleaned)),
            None => Err(curves::OCCError::new_err(
                "removeSplitter: the kernel could not unify the shape",
            )),
        }
    }

    /// `Shape.makeFillet(radius, edgeList)` — round the given edges.
    ///
    /// Upstream's two-radius (variable-radius) form is accepted in the
    /// signature but not implemented.
    #[pyo3(signature = (*args))]
    fn makeFillet(&self, args: &Bound<'_, PyTuple>) -> PyResult<PartShape> {
        let (radius, edges) = match args.len() {
            2 => (args.get_item(0)?.extract::<f64>()?, args.get_item(1)?),
            3 => {
                return Err(curves::OCCError::new_err(
                    "makeFillet: variable-radius fillets are not implemented yet",
                ))
            }
            _ => {
                return Err(PyValueError::new_err(
                    "makeFillet takes (radius, edgeList) or (radius1, radius2, edgeList)",
                ))
            }
        };
        if !(radius > 0.0) {
            return Err(curves::OCCError::new_err(
                "makeFillet needs a positive radius",
            ));
        }
        let shape = self.named_shape()?;
        let edges = shape_sequence(&edges)?;
        if edges.is_empty() {
            return Err(curves::OCCError::new_err(
                "makeFillet needs at least one edge",
            ));
        }
        let references = edge_references(&shape, &edges);
        let result = ferrocad_part::backend()
            .fillet(&shape, &references, radius)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(result.shape))
    }

    /// `Shape.makeChamfer(distance, edgeList)` — bevel the given edges.
    ///
    /// `makeChamfer(distance1, distance2, edgeList)` is the two-distance form.
    #[pyo3(signature = (*args))]
    fn makeChamfer(&self, args: &Bound<'_, PyTuple>) -> PyResult<PartShape> {
        let (distance1, distance2, edges) = match args.len() {
            2 => (args.get_item(0)?.extract::<f64>()?, None, args.get_item(1)?),
            3 => (
                args.get_item(0)?.extract::<f64>()?,
                Some(args.get_item(1)?.extract::<f64>()?),
                args.get_item(2)?,
            ),
            _ => {
                return Err(PyValueError::new_err(
                    "makeChamfer takes (distance, edgeList) or (distance1, distance2, edgeList)",
                ))
            }
        };
        if !(distance1 > 0.0) || distance2.map(|d| !(d > 0.0)).unwrap_or(false) {
            return Err(curves::OCCError::new_err(
                "makeChamfer needs positive distances",
            ));
        }
        let shape = self.named_shape()?;
        let edges = shape_sequence(&edges)?;
        if edges.is_empty() {
            return Err(curves::OCCError::new_err(
                "makeChamfer needs at least one edge",
            ));
        }
        let references = edge_references(&shape, &edges);
        let result = ferrocad_part::backend()
            .chamfer(&shape, &references, distance1, distance2)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(result.shape))
    }

    /// `Shape.section(tool, tolerance=0, approximation=False)` — the
    /// intersection curve(s) of this shape and `tool`.
    ///
    /// `tolerance` and `approximation` are accepted for signature compatibility
    /// and ignored (the kernel runs the exact section).
    #[pyo3(signature = (tool, tolerance = 0.0, approximation = false))]
    fn section(
        &self,
        tool: &Bound<'_, PyAny>,
        tolerance: f64,
        approximation: bool,
    ) -> PyResult<PartShape> {
        let _ = (tolerance, approximation);
        let shape = self.named_shape()?;
        let tools = shape_sequence(tool)?;
        if tools.is_empty() {
            return Err(curves::OCCError::new_err("section needs a tool shape"));
        }
        let backend = ferrocad_part::backend();
        let mut sections = Vec::with_capacity(tools.len());
        for tool in &tools {
            sections.push(backend.section(&shape, tool).map_err(kernel_error)?);
        }
        let result = if sections.len() == 1 {
            sections.remove(0)
        } else {
            backend.make_compound(&sections).map_err(kernel_error)?
        };
        Ok(PartShape::from_shape(result))
    }

    /// `Shape.project(shapes)` — project `shapes` onto this shape.
    ///
    /// This slice supports a **planar** target only: a tool that already lies in
    /// the target's plane projects onto itself, so the result is a compound of the
    /// tools. Projecting onto any other surface raises `Part.OCCError`.
    fn project(&self, shapes: &Bound<'_, PyAny>) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        let tools = shape_sequence(shapes)?;
        let projected = ferrocad_part::backend()
            .project(&shape, &tools)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(projected))
    }

    /// `Shape.slice(direction, distance)` — the wires where the shape meets the
    /// plane `direction . x = distance` (upstream's parameterisation: the
    /// direction is not normalised).
    fn slice(
        &self,
        py: Python<'_>,
        direction: &Bound<'_, PyAny>,
        distance: f64,
    ) -> PyResult<Vec<Py<PartShape>>> {
        let shape = self.named_shape()?;
        let direction = curves::vector(direction)?;
        let wires = ferrocad_part::backend()
            .slice(&shape, direction, distance)
            .map_err(kernel_error)?;
        wires
            .into_iter()
            .map(|wire| Py::new(py, PartShape::from_shape(wire)))
            .collect()
    }

    /// `Shape.slices(direction, distancesList)` — a compound of the wires at
    /// every distance.
    fn slices(
        &self,
        direction: &Bound<'_, PyAny>,
        distances: &Bound<'_, PyAny>,
    ) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        let direction = curves::vector(direction)?;
        let backend = ferrocad_part::backend();
        let mut wires = Vec::new();
        for value in distances.try_iter()? {
            let distance: f64 = value?.extract()?;
            wires.extend(backend.slice(&shape, direction, distance).map_err(kernel_error)?);
        }
        if wires.is_empty() {
            return Err(curves::OCCError::new_err("slices produced no wires"));
        }
        Ok(PartShape::from_shape(
            backend.make_compound(&wires).map_err(kernel_error)?,
        ))
    }

    /// `Shape.generalFuse(shapes, fuzzy_value=0)` — General Fuse of this shape
    /// and `shapes`.
    ///
    /// Returns upstream's `(result, map)`: the combined shape and, for each
    /// input (this shape first), the parts it was split into.
    #[pyo3(signature = (shapes, fuzzy_value = 0.0))]
    fn generalFuse(
        &self,
        py: Python<'_>,
        shapes: &Bound<'_, PyAny>,
        fuzzy_value: f64,
    ) -> PyResult<(Py<PartShape>, Vec<Vec<Py<PartShape>>>)> {
        let _ = fuzzy_value;
        let first = self.named_shape()?;
        let mut inputs = vec![first];
        inputs.extend(shape_sequence(shapes)?);
        let (result, split) = ferrocad_part::backend()
            .general_fuse(&inputs)
            .map_err(kernel_error)?;
        let result = Py::new(py, PartShape::from_shape(result))?;
        let split = split
            .into_iter()
            .map(|parts| {
                parts
                    .into_iter()
                    .map(|shape| Py::new(py, PartShape::from_shape(shape)))
                    .collect::<PyResult<Vec<_>>>()
            })
            .collect::<PyResult<Vec<_>>>()?;
        Ok((result, split))
    }

    /// `Shape.optimalBoundingBox(useTriangulation=True,
    /// useShapeTolerance=False)` — `BRepBndLib::AddOptimal` bounds.
    #[pyo3(signature = (useTriangulation = true, useShapeTolerance = false))]
    fn optimalBoundingBox(
        &self,
        py: Python<'_>,
        useTriangulation: bool,
        useShapeTolerance: bool,
    ) -> PyResult<PyObject> {
        let module = py.import("ferrocad")?;
        let bound_box = module.getattr("BoundBox")?;
        let Some(shape) = self.shape() else {
            return Ok(bound_box.call0()?.unbind());
        };
        match ferrocad_part::backend().optimal_bounds(&shape, useTriangulation, useShapeTolerance) {
            Some((min, max)) => Ok(bound_box
                .call1((min.x, min.y, min.z, max.x, max.y, max.z))?
                .unbind()),
            None => Ok(bound_box.call0()?.unbind()),
        }
    }

    /// `Shape.getChildShapes(name="")` — the direct children whose name
    /// matches `name` (an exact sub-element name such as `"Solid1"`, or `""` /
    /// `"*"` for all of them).
    fn getChildShapes(&self, py: Python<'_>, name: &str) -> PyResult<Vec<Py<PartShape>>> {
        let shape = self.named_shape()?;
        if !name.is_empty() && name != "*" {
            let Some((kind, _)) = sub_shape_name(name) else {
                return Ok(Vec::new());
            };
            let elements = ferrocad_part::backend().elements(&shape, kind);
            return elements
                .into_iter()
                .filter(|element| element.name == name)
                .map(|element| Py::new(py, PartShape::from_shape(element.shape)))
                .collect();
        }
        for (kind, _) in ELEMENT_KINDS {
            let shapes = self.sub_shapes(kind)?;
            if !shapes.is_empty() {
                return shapes
                    .into_iter()
                    .map(|shape| Py::new(py, PartShape::from_shape(shape)))
                    .collect();
            }
        }
        Ok(Vec::new())
    }

    /// `Shape.findSubShapesWithSharedVertex(shape, needName=False, ...)` — the
    /// sub-shapes of the same kind as `shape` whose vertex set matches it within
    /// `tol` (upstream also compares the geometry; this kernel compares the
    /// vertices).
    #[pyo3(signature = (shape, needName = false, checkGeometry = true, tol = 1e-7, atol = 1e-12, singleResult = false))]
    fn findSubShapesWithSharedVertex(
        &self,
        py: Python<'_>,
        shape: &Bound<'_, PyAny>,
        needName: bool,
        checkGeometry: bool,
        tol: f64,
        atol: f64,
        singleResult: bool,
    ) -> PyResult<PyObject> {
        let _ = (checkGeometry, atol);
        let target = shape_argument(shape)?;
        let mut found: Vec<(String, Shape)> = Vec::new();
        if let Some(kind) = PartShape::kind_of(&target) {
            let Some(own) = self.shape() else {
                return Ok(PyList::empty(py).into_any().unbind());
            };
            let backend = ferrocad_part::backend();
            let target_points = vertex_points(&target);
            for element in backend.elements(&own, kind) {
                let matches = if target_points.is_empty() {
                    backend.same_shape(&element.shape, &target)
                } else {
                    same_vertex_set(&target_points, &vertex_points(&element.shape), tol)
                };
                if !matches {
                    continue;
                }
                found.push((element.name, element.shape));
                if singleResult {
                    break;
                }
            }
        }
        if needName {
            let mut items = Vec::with_capacity(found.len());
            for (name, shape) in found {
                items.push((name, Py::new(py, PartShape::from_shape(shape))?));
            }
            return Ok(PyList::new(py, items)?.into_any().unbind());
        }
        let items = found
            .into_iter()
            .map(|(_, shape)| Py::new(py, PartShape::from_shape(shape)))
            .collect::<PyResult<Vec<_>>>()?;
        Ok(PyList::new(py, items)?.into_any().unbind())
    }

    /// `Shape.getElement(name)` — resolve a sub-element name (`Face2`) to the
    /// sub-shape. Without an element map this is the positional resolution.
    fn getElement(&self, name: &str) -> PyResult<PartShape> {
        let Some(shape) = self.shape() else {
            return Ok(PartShape::empty());
        };
        match ferrocad_part::backend().resolve(&shape, name) {
            Some(resolved) => Ok(PartShape::from_shape(resolved)),
            None => Ok(PartShape::empty()),
        }
    }

    /// `Shape.findSubShape(shape)` — the `(kind, index)` of `shape` inside this
    /// one, or `(None, 0)` when it is not a sub-shape; a sequence of shapes gives
    /// a list of those tuples.
    fn findSubShape(&self, value: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let py = value.py();
        if value.downcast::<PartShape>().is_ok() {
            let target = shape_argument(value)?;
            return match self.locate(&target) {
                Some((kind, index)) => (kind, index).into_py_any(py),
                None => (Option::<&str>::None, 0usize).into_py_any(py),
            };
        }
        let mut found = Vec::new();
        for item in value.try_iter()? {
            let target = shape_argument(&item?)?;
            match self.locate(&target) {
                Some((kind, index)) => found.push((Some(kind), index)),
                None => found.push((None, 0)),
            }
        }
        Ok(PyList::new(py, found)?.into_any().unbind())
    }

    /// `Shape.removeShape(shapes)` — a copy with the given sub-shapes removed
    /// (`BRepTools_ReShape`).
    fn removeShape(&self, shapes: &Bound<'_, PyAny>) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        let removals = shape_sequence(shapes)?;
        if removals.is_empty() {
            return Ok(self.copy_handle());
        }
        let result = ferrocad_part::backend()
            .remove_shapes(&shape, &removals)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(result))
    }

    /// `Shape.replaceShape([(old, new), ...])` — a copy with each `old`
    /// sub-shape replaced by `new` (`BRepTools_ReShape`).
    fn replaceShape(&self, replacements: &Bound<'_, PyAny>) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        let mut pairs = Vec::new();
        for item in replacements.try_iter()? {
            let item = item?;
            let old = shape_argument(&item.get_item(0)?)?;
            let new = shape_argument(&item.get_item(1)?)?;
            pairs.push((old, new));
        }
        if pairs.is_empty() {
            return Ok(self.copy_handle());
        }
        let result = ferrocad_part::backend()
            .replace_shapes(&shape, &pairs)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(result))
    }

    /// `Edge.split(paramval)` — split this edge at one parameter or a list of
    /// parameters and return the pieces as a wire, in parameter order.
    ///
    /// `paramval` is a curve parameter on the edge's basis curve, as upstream's
    /// parameterisation; each value must lie strictly inside the edge's range.
    /// The original edge is not modified. Defined on `Part.Shape` so `Part.Edge`
    /// inherits it (upstream has it on `Edge` only).
    fn split(&self, paramval: &Bound<'_, PyAny>) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        let mut parameters = Vec::new();
        if let Ok(value) = paramval.extract::<f64>() {
            parameters.push(value);
        } else if let Ok(values) = paramval.try_iter() {
            for value in values {
                parameters.push(value?.extract::<f64>()?);
            }
        } else {
            return Err(pyo3::exceptions::PyTypeError::new_err(
                "Either float or list of floats expected",
            ));
        }
        let split = ferrocad_part::backend()
            .split_edge(&shape, &parameters)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(split))
    }

    /// `Shape.mapSubElement(shape, op=None)` — upstream's hook for building an
    /// element map from a sub-shape selection.
    ///
    /// **This kernel emits no element maps** (`ElementMapVersion` is `""`), so
    /// there is nothing to build: the call is a documented no-op and returns
    /// this shape unchanged. Upstream's own tests skip the map assertions when
    /// the version is empty, so callers that guard on it behave identically.
    #[pyo3(signature = (shape, op = None))]
    fn mapSubElement<'py>(
        slf: &Bound<'py, Self>,
        shape: &Bound<'_, PyAny>,
        op: Option<&str>,
    ) -> PyResult<PyObject> {
        let _ = (shape, op);
        Ok(slf.clone().into_any().unbind())
    }

    /// `Shape.reverse()` — reverse this shape's orientation in place.
    fn reverse(slf: &Bound<'_, Self>) -> PyResult<()> {
        let shape = slf.borrow().named_shape()?;
        let reversed = ferrocad_part::backend()
            .reversed(&shape)
            .map_err(kernel_error)?;
        slf.borrow_mut().replace_shape(Some(reversed));
        Ok(())
    }

    /// `Shape.reversed()` — a copy with the reversed orientation.
    fn reversed(&self) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        let reversed = ferrocad_part::backend()
            .reversed(&shape)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(reversed))
    }
}

/// The sub-shape kinds, highest dimension first, with their FreeCAD names.
const ELEMENT_KINDS: [(ElementKind, &str); 6] = [
    (ElementKind::Solid, "Solid"),
    (ElementKind::Shell, "Shell"),
    (ElementKind::Face, "Face"),
    (ElementKind::Wire, "Wire"),
    (ElementKind::Edge, "Edge"),
    (ElementKind::Vertex, "Vertex"),
];

/// Split `Face3` into `(ElementKind::Face, 2)`.
fn sub_shape_name(name: &str) -> Option<(ElementKind, usize)> {
    let (prefix, digits) = ["CompSolid", "Solid", "Shell", "Face", "Wire", "Edge", "Vertex"]
        .into_iter()
        .find_map(|prefix| name.strip_prefix(prefix).map(|digits| (prefix, digits)))?;
    let index: usize = digits.parse().ok()?;
    if index == 0 {
        return None;
    }
    let kind = match prefix {
        "Solid" => ElementKind::Solid,
        "Shell" => ElementKind::Shell,
        "Face" => ElementKind::Face,
        "Wire" => ElementKind::Wire,
        "Edge" => ElementKind::Edge,
        "Vertex" => ElementKind::Vertex,
        // The seam has no comp-solid kind yet.
        _ => return None,
    };
    Some((kind, index - 1))
}

/// Map a kernel failure to `Part.OCCError`.
fn kernel_error(error: ferrocad_geom::GeomError) -> PyErr {
    curves::OCCError::new_err(error.to_string())
}

fn translation_matrix(offset: ferrocad_curves::Vector3) -> ferrocad_types::Matrix4 {
    let mut values = ferrocad_types::Matrix4::identity().m;
    values[3] = offset.x;
    values[7] = offset.y;
    values[11] = offset.z;
    ferrocad_types::Matrix4::from_values(values)
}

fn scale_matrix(factor: f64, center: ferrocad_curves::Vector3) -> ferrocad_types::Matrix4 {
    let mut values = ferrocad_types::Matrix4::identity().m;
    values[0] = factor;
    values[5] = factor;
    values[10] = factor;
    values[3] = center.x * (1.0 - factor);
    values[7] = center.y * (1.0 - factor);
    values[11] = center.z * (1.0 - factor);
    ferrocad_types::Matrix4::from_values(values)
}

fn mirror_matrix(
    point: ferrocad_curves::Vector3,
    normal: ferrocad_curves::Vector3,
) -> PyResult<ferrocad_types::Matrix4> {
    if normal.length() == 0.0 {
        return Err(curves::OCCError::new_err(
            "a mirror plane needs a non-zero normal",
        ));
    }
    let n = normal.normalize();
    let d = n.x * point.x + n.y * point.y + n.z * point.z;
    let mut values = ferrocad_types::Matrix4::identity().m;
    values[0] = 1.0 - 2.0 * n.x * n.x;
    values[1] = -2.0 * n.x * n.y;
    values[2] = -2.0 * n.x * n.z;
    values[3] = 2.0 * d * n.x;
    values[4] = values[1];
    values[5] = 1.0 - 2.0 * n.y * n.y;
    values[6] = -2.0 * n.y * n.z;
    values[7] = 2.0 * d * n.y;
    values[8] = values[2];
    values[9] = values[6];
    values[10] = 1.0 - 2.0 * n.z * n.z;
    values[11] = 2.0 * d * n.z;
    Ok(ferrocad_types::Matrix4::from_values(values))
}

/// Read a `ferrocad.Matrix` through its `A11`…`A44` elements.
fn matrix_from_py(value: &Bound<'_, PyAny>) -> PyResult<ferrocad_types::Matrix4> {
    let mut values = [0.0; 16];
    for row in 1..=4 {
        for column in 1..=4 {
            let name = format!("A{row}{column}");
            values[(row - 1) * 4 + (column - 1)] = value.getattr(name)?.extract()?;
        }
    }
    Ok(ferrocad_types::Matrix4::from_values(values))
}

/// The shape wrapped by a Python `Part.Shape` (or one of its subclasses).
pub(crate) fn shape_argument(value: &Bound<'_, PyAny>) -> PyResult<Shape> {
    let shape = value.downcast::<PartShape>().map_err(|_| {
        pyo3::exceptions::PyTypeError::new_err("expected a Part.Shape")
    })?;
    shape
        .borrow()
        .shape()
        .ok_or_else(|| curves::OCCError::new_err("a null shape cannot be combined"))
}

/// The shapes in `value`: a single `Part.Shape` (or subclass) or an iterable of
/// them, as upstream's `getPyShapes` accepts both.
fn shape_sequence(value: &Bound<'_, PyAny>) -> PyResult<Vec<Shape>> {
    if value.downcast::<PartShape>().is_ok() {
        return Ok(vec![shape_argument(value)?]);
    }
    let mut shapes = Vec::new();
    for item in value.try_iter()? {
        shapes.push(shape_argument(&item?)?);
    }
    Ok(shapes)
}

/// Edge references for a shape-level fillet or chamfer: each given shape is
/// resolved to the matching `EdgeN` of `target` when it is one of its edges, so
/// the operation sees the target's own topology rather than a foreign copy.
fn edge_references(target: &Shape, edges: &[Shape]) -> Vec<ElementRef> {
    let backend = ferrocad_part::backend();
    let available = backend.elements(target, ElementKind::Edge);
    edges
        .iter()
        .enumerate()
        .map(|(position, edge)| {
            available
                .iter()
                .find(|element| backend.same_shape(&element.shape, edge))
                .cloned()
                .unwrap_or_else(|| ElementRef {
                    shape: edge.clone(),
                    name: format!("Edge{}", position + 1),
                })
        })
        .collect()
}

/// The points of a shape's vertices, in the kernel's order.
fn vertex_points(shape: &Shape) -> Vec<ferrocad_curves::Vector3> {
    let backend = ferrocad_part::backend();
    backend
        .elements(shape, ElementKind::Vertex)
        .into_iter()
        .filter_map(|vertex| backend.vertex_point(&vertex.shape))
        .collect()
}

/// Whether two vertex sets are equal within `tolerance` (a bijective match, both
/// ways, so duplicate points are handled correctly).
fn same_vertex_set(
    first: &[ferrocad_curves::Vector3],
    second: &[ferrocad_curves::Vector3],
    tolerance: f64,
) -> bool {
    if first.len() != second.len() {
        return false;
    }
    let mut unmatched = second.to_vec();
    for point in first {
        let Some(position) = unmatched.iter().position(|other| point.distance(other) <= tolerance)
        else {
            return false;
        };
        unmatched.remove(position);
    }
    unmatched.is_empty()
}

enum BooleanOp {
    Fuse,
    Cut,
    Common,
}

impl PartShape {
    fn transformed_by(&self, matrix: &ferrocad_types::Matrix4) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        let moved = ferrocad_part::backend()
            .transform(&shape, matrix)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(moved))
    }

    fn mass_properties(&self) -> Option<ferrocad_geom::MassProperties> {
        let shape = self.shape()?;
        ferrocad_part::backend().mass_properties(&shape)
    }

    fn named_shape(&self) -> PyResult<Shape> {
        self.shape()
            .ok_or_else(|| curves::OCCError::new_err("a null shape cannot be combined"))
    }

    fn boolean(&self, other: &Bound<'_, PyAny>, operation: BooleanOp) -> PyResult<PartShape> {
        let a = self.named_shape()?;
        let b = shape_argument(other)?;
        let backend = ferrocad_part::backend();
        let result = match operation {
            BooleanOp::Fuse => backend.fuse(&a, &b),
            BooleanOp::Cut => backend.cut(&a, &b),
            BooleanOp::Common => backend.common(&a, &b),
        };
        Ok(PartShape::from_shape(result.map_err(kernel_error)?.shape))
    }
}

/// `Part.makeCylinder(radius, height, pnt=None, dir=None, angle=360)`.
#[pyfunction]
#[pyo3(signature = (radius, height, pnt = None, dir = None, angle = 360.0))]
fn makeCylinder(
    radius: f64,
    height: f64,
    pnt: Option<&Bound<'_, PyAny>>,
    dir: Option<&Bound<'_, PyAny>>,
    angle: f64,
) -> PyResult<PartShape> {
    require_full_revolution("makeCylinder", angle)?;
    let base = pnt
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(ferrocad_curves::Vector3::zero);
    let direction = dir
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(|| ferrocad_curves::Vector3::new(0.0, 0.0, 1.0));
    let shape = ferrocad_part::backend()
        .make_cylinder(radius, height, base, direction)
        .map_err(kernel_error)?;
    Ok(PartShape::from_shape(shape))
}

/// `Part.makeSphere(radius, center=None, dir=None, angle1=-90, angle2=90, angle3=360)`.
#[pyfunction]
#[pyo3(signature = (radius, center = None, dir = None, angle1 = -90.0, angle2 = 90.0, angle3 = 360.0))]
fn makeSphere(
    radius: f64,
    center: Option<&Bound<'_, PyAny>>,
    dir: Option<&Bound<'_, PyAny>>,
    angle1: f64,
    angle2: f64,
    angle3: f64,
) -> PyResult<PartShape> {
    let _ = dir;
    if (angle1 + 90.0).abs() > 1e-9 || (angle2 - 90.0).abs() > 1e-9 {
        return Err(curves::OCCError::new_err(
            "partial spheres are not implemented yet",
        ));
    }
    require_full_revolution("makeSphere", angle3)?;
    let center = center
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(ferrocad_curves::Vector3::zero);
    let shape = ferrocad_part::backend()
        .make_sphere(radius, center)
        .map_err(kernel_error)?;
    Ok(PartShape::from_shape(shape))
}

/// `Part.makeCone(radius1, radius2, height, pnt=None, dir=None, angle=360)`.
#[pyfunction]
#[pyo3(signature = (radius1, radius2, height, pnt = None, dir = None, angle = 360.0))]
fn makeCone(
    radius1: f64,
    radius2: f64,
    height: f64,
    pnt: Option<&Bound<'_, PyAny>>,
    dir: Option<&Bound<'_, PyAny>>,
    angle: f64,
) -> PyResult<PartShape> {
    require_full_revolution("makeCone", angle)?;
    let base = pnt
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(ferrocad_curves::Vector3::zero);
    let direction = dir
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(|| ferrocad_curves::Vector3::new(0.0, 0.0, 1.0));
    let shape = ferrocad_part::backend()
        .make_cone(radius1, radius2, height, base, direction)
        .map_err(kernel_error)?;
    Ok(PartShape::from_shape(shape))
}

fn require_full_revolution(operation: &str, angle: f64) -> PyResult<()> {
    if (angle - 360.0).abs() > 1e-9 {
        return Err(curves::OCCError::new_err(format!(
            "{operation}: partial revolutions are not implemented yet"
        )));
    }
    Ok(())
}

/// `Part.makeBox(length, width, height, pnt=None, dir=None)` → a `Part.Shape`.
#[pyfunction]
#[pyo3(signature = (length, width, height, pnt = None, dir = None))]
fn makeBox(
    length: f64,
    width: f64,
    height: f64,
    pnt: Option<&Bound<'_, PyAny>>,
    dir: Option<&Bound<'_, PyAny>>,
) -> PyResult<PartShape> {
    let mut shape = ferrocad_part::make_box(length, width, height)
        .ok_or_else(|| PyRuntimeError::new_err("Part.makeBox did not produce a shape"))?;
    if pnt.is_some() || dir.is_some() {
        let base = pnt
            .map(curves::vector)
            .transpose()?
            .unwrap_or_else(ferrocad_curves::Vector3::zero);
        let rotation = match dir.map(curves::vector).transpose()? {
            Some(direction) if direction.length() > 0.0 => {
                ferrocad_types::Rotation::from_vectors(
                    ferrocad_curves::Vector3::new(0.0, 0.0, 1.0),
                    direction,
                )
            }
            _ => ferrocad_types::Rotation::identity(),
        };
        shape = ferrocad_part::backend()
            .place(&shape, &ferrocad_types::Placement { base, rotation })
            .map_err(kernel_error)?;
    }
    Ok(PartShape::from_shape(shape))
}

/// `Part.makeSplitShape(shape, splits, checkInterior=True)` → upstream's
/// `(direct, left)` tuple of two lists of shapes.
///
/// Each `splits` pair is `(splitting shape, face being split)`: an edge or wire
/// cuts the face it lies on (`BRepFeat_SplitShape`). The first list holds the
/// pieces the split generated directly (`DirectLeft`), the second the pieces left
/// over (`Left`), matching upstream's grouping and order.
#[pyfunction]
#[pyo3(signature = (shape, splits, checkInterior = true))]
fn makeSplitShape(
    py: Python<'_>,
    shape: &Bound<'_, PyAny>,
    splits: &Bound<'_, PyAny>,
    checkInterior: bool,
) -> PyResult<PyObject> {
    let target = shape_argument(shape)?;
    let mut tools = Vec::new();
    for item in splits.try_iter()? {
        let item = item?;
        let cutter = shape_argument(&item.get_item(0)?)?;
        let face = shape_argument(&item.get_item(1)?)?;
        tools.push((cutter, face));
    }
    let (direct, left) = ferrocad_part::backend()
        .split_shape(&target, &tools, checkInterior)
        .map_err(kernel_error)?;
    let direct: Vec<PartShape> = direct.into_iter().map(PartShape::from_shape).collect();
    let left: Vec<PartShape> = left.into_iter().map(PartShape::from_shape).collect();
    let direct = PyList::new(py, direct)?;
    let left = PyList::new(py, left)?;
    Ok(PyTuple::new(py, [direct, left])?.into_any().unbind())
}

/// `Part.makeSweepSurface(path, profile, tolerance=0.001, fillMode=0)` — sweep
/// `profile` along `path`.
///
/// `path` may be a wire or a single edge. `tolerance` and `fillMode` are
/// accepted for upstream signature compatibility; the pipe is built with
/// `BRepOffsetAPI_MakePipe`, which has no approximation tolerance, so the result
/// is the exact swept surface.
#[pyfunction]
#[pyo3(signature = (path, profile, tolerance = 0.001, fillMode = 0))]
fn makeSweepSurface(
    path: &Bound<'_, PyAny>,
    profile: &Bound<'_, PyAny>,
    tolerance: f64,
    fillMode: i32,
) -> PyResult<PartShape> {
    let _ = fillMode;
    let spine = shape_argument(path)?;
    let profile = shape_argument(profile)?;
    let piped = ferrocad_part::backend()
        .make_pipe(&spine, &profile, tolerance)
        .map_err(kernel_error)?;
    Ok(PartShape::from_shape(piped))
}

/// `Part.show(shape, name="Shape")` — add a `Part::Feature` for `shape` to the
/// active document (creating a document when there is none) and return it.
#[pyfunction]
#[pyo3(signature = (shape, name = None))]
fn show(py: Python<'_>, shape: &Bound<'_, PyAny>, name: Option<&str>) -> PyResult<PyObject> {
    let shape = shape_argument(shape)?;
    let app = py.import("ferrocad")?;
    let active = app.call_method0("activeDocument")?;
    let document = if active.is_none() {
        app.call_method0("newDocument")?
    } else {
        active
    };
    let object = document.call_method1(
        "addObject",
        ("Part::Feature", name.unwrap_or("Shape")),
    )?;
    object.setattr("Shape", PartShape::from_shape(shape))?;
    Ok(object.unbind())
}

/// `Part.getSortedClusters(edges)` — group edges into connected clusters.
///
/// The kernel reports no adjacency between loose edges, so the clusters are
/// found from the edges' endpoint vertices: two edges join when they share a
/// vertex within `ferrocad_curves::TOLERANCE` (the kernel's confusion
/// tolerance). The clusters are returned in the order their first edge appears
/// in the input, each listing its edges in that same order.
#[pyfunction]
fn getSortedClusters(py: Python<'_>, edges: &Bound<'_, PyAny>) -> PyResult<Py<PyList>> {
    let backend = ferrocad_part::backend();
    let mut shapes = Vec::new();
    for edge in edges.try_iter()? {
        shapes.push(shape_argument(&edge?)?);
    }

    // Each edge's endpoints, read from its vertex sub-shapes.
    let endpoints: Vec<Vec<ferrocad_curves::Vector3>> = shapes
        .iter()
        .map(|shape| {
            backend
                .elements(shape, ElementKind::Vertex)
                .into_iter()
                .filter_map(|vertex| backend.vertex_point(&vertex.shape))
                .collect()
        })
        .collect();

    let mut parent: Vec<usize> = (0..shapes.len()).collect();
    for i in 0..shapes.len() {
        for j in (i + 1)..shapes.len() {
            let connected = endpoints[i].iter().any(|a| {
                endpoints[j]
                    .iter()
                    .any(|b| a.distance(b) <= ferrocad_curves::TOLERANCE)
            });
            if connected {
                cluster_union(&mut parent, i, j);
            }
        }
    }

    let mut roots: Vec<usize> = Vec::new();
    let mut clusters: Vec<Vec<Py<PartShape>>> = Vec::new();
    for index in 0..shapes.len() {
        let root = cluster_find(&mut parent, index);
        let slot = match roots.iter().position(|known| *known == root) {
            Some(slot) => slot,
            None => {
                roots.push(root);
                clusters.push(Vec::new());
                clusters.len() - 1
            }
        };
        clusters[slot].push(Py::new(py, PartShape::from_shape(shapes[index].clone()))?);
    }

    let lists = clusters
        .into_iter()
        .map(|cluster| PyList::new(py, cluster))
        .collect::<PyResult<Vec<_>>>()?;
    Ok(PyList::new(py, lists)?.unbind())
}

/// The representative of `index`'s cluster, with path compression.
fn cluster_find(parent: &mut [usize], index: usize) -> usize {
    let mut root = index;
    while parent[root] != root {
        root = parent[root];
    }
    let mut current = index;
    while parent[current] != root {
        let next = parent[current];
        parent[current] = root;
        current = next;
    }
    root
}

fn cluster_union(parent: &mut [usize], a: usize, b: usize) {
    let root_a = cluster_find(parent, a);
    let root_b = cluster_find(parent, b);
    if root_a != root_b {
        parent[root_b] = root_a;
    }
}

/// `Part::PropertyPartShape` → `Part.Shape` (what `obj.Shape` returns).
fn shape_to_py(py: Python<'_>, data: &dyn ExtensionData) -> Option<PyObject> {
    let property = data.as_any().downcast_ref::<ShapeProperty>()?;
    let shape = property.shape();
    Py::new(py, PartShape { shape: Mutex::new(shape) })
        .ok()
        .map(Py::into_any)
}

/// `Part::PropertyTopoShapeList` → a Python list of `Part.Shape` (what `obj.Shapes`
/// returns).
fn shape_list_to_py(py: Python<'_>, data: &dyn ExtensionData) -> Option<PyObject> {
    let property = data.as_any().downcast_ref::<ShapeListProperty>()?;
    let items = property
        .shapes()
        .into_iter()
        .map(|shape| Py::new(py, PartShape { shape: Mutex::new(Some(shape)) }))
        .collect::<PyResult<Vec<_>>>()
        .ok()?;
    let list = PyList::new(py, items).ok()?;
    Some(list.into_any().unbind())
}

/// `Part.Shape` → `Part::PropertyPartShape` (what `obj.Shape = shape` stores).
fn shape_to_extension(value: &Bound<'_, PyAny>) -> Option<ExtensionValue> {
    let shape = value.downcast::<PartShape>().ok()?;
    let inner = shape.borrow().shape();
    let property = match inner {
        Some(shape) => ShapeProperty::new(shape),
        None => ShapeProperty::empty(),
    };
    Some(ExtensionValue::new(Box::new(property)))
}

/// A list of `Part.Shape` → `Part::PropertyTopoShapeList`.
///
/// `None` for anything that is not a sequence of shapes, and also for a sequence
/// holding a null `Part.Shape()`: the seam has no null shape, so such an entry
/// cannot be stored (assigning it fails rather than dropping or reordering it).
fn shape_list_to_extension(value: &Bound<'_, PyAny>) -> Option<ExtensionValue> {
    let items = value.extract::<Vec<PyRef<'_, PartShape>>>().ok()?;
    let mut shapes = Vec::with_capacity(items.len());
    for item in items {
        shapes.push(item.shape()?);
    }
    Some(ExtensionValue::new(Box::new(ShapeListProperty::new(shapes))))
}

#[pymodule]
pub fn Part(m: &Bound<'_, PyModule>) -> PyResult<()> {
    register_kernel();
    ferrocad_py::register_extension_converter(TYPE_NAME, shape_to_py, shape_to_extension);
    ferrocad_py::register_extension_converter(
        SHAPE_LIST_TYPE_NAME,
        shape_list_to_py,
        shape_list_to_extension,
    );
    m.add_class::<PartShape>()?;
    m.add_function(wrap_pyfunction!(makeBox, m)?)?;
    m.add_function(wrap_pyfunction!(makeCylinder, m)?)?;
    m.add_function(wrap_pyfunction!(makeSphere, m)?)?;
    m.add_function(wrap_pyfunction!(makeCone, m)?)?;
    m.add_function(wrap_pyfunction!(makeSplitShape, m)?)?;
    m.add_function(wrap_pyfunction!(makeSweepSurface, m)?)?;
    m.add_function(wrap_pyfunction!(show, m)?)?;
    m.add_function(wrap_pyfunction!(getSortedClusters, m)?)?;
    curves::register(m)?;
    containers::register(m)?;
    solids::register(m)?;
    Ok(())
}
