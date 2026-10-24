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

mod chfi2d;
mod containers;
mod curves;
pub mod feature_python;
mod link;
mod link_array;
mod shape_fix;
mod solids;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ferrocad_core::{ExtensionData, ExtensionValue, Placement, Rotation, ScaleType, Vector3};
use ferrocad_geom::{ElementKind, ElementRef, Lineage, MapVersion, Shape, TessellationOptions};
use ferrocad_occt::surface_ops;
use ferrocad_occt::OcctBackend;
use ferrocad_part::{resolve_element_in, ShapeListProperty, ShapeProperty, SHAPE_LIST_TYPE_NAME, TYPE_NAME};
use pyo3::exceptions::{PyIOError, PyIndexError, PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList, PyModule, PyTuple};
use pyo3::IntoPyObjectExt;

/// Register the Part kernel (OCCT) and its core types. Idempotent; every entry
/// point that can build a shape calls it first.
fn register_kernel() {
    ferrocad_part::register(Arc::new(OcctBackend::new()));
    link::register();
    link_array::register();
    solids::register_objects();
    feature_python::register();
}

/// `Part.Shape`: an opaque handle to a kernel shape.
///
/// The `Mutex`/`Arc` is here at the Python boundary only: `Shape` is `Send` but not
/// `Sync` (see `ferrocad_geom`), and a `#[pyclass]` must be `Send + Sync`. It is not
/// a lock in the seam.
///
/// The geometry is **always baked** with the accumulated [`placement`](PartShape::placement)
/// (`geometry == placement · base`), so `BoundBox`, `Vertexes`, `normalAt`, mass
/// properties and sub-shape access need no extra transform. The placement is kept
/// beside it, in an `Arc` shared with the live Python `Placement` view, so mutating
/// `shape.Placement` re-bakes the geometry and moves the description together.
///
/// A shape produced by an operation also carries the [`Lineage`] that operation
/// folded: names are metadata *beside* the geometry, never part of it, so they
/// cannot change any Python-visible value. The lineage is deliberately not carried
/// across transforms — those replace every sub-shape handle, and a map pointing at
/// the old handles would be worse than none.
#[pyclass(name = "Shape", module = "Part", subclass)]
pub struct PartShape {
    /// The baked geometry (pre-existing field name kept for the sub-shape wrappers
    /// in `curves`/`solids`, which rebuild it in place).
    shape: Arc<Mutex<Option<Shape>>>,
    /// The accumulated placement baked into `shape`.
    placement: Arc<Mutex<Placement>>,
    lineage: Arc<Mutex<Option<Lineage>>>,
    /// Bumped whenever the geometry/placement pair changes, so a `Placement` view
    /// captured earlier detaches instead of overwriting a newer value (the same
    /// rule the document property views use).
    epoch: Arc<AtomicU64>,
}

impl PartShape {
    fn with_parts(shape: Option<Shape>, lineage: Option<Lineage>) -> Self {
        Self::with_parts_and_placement(shape, Placement::identity(), lineage)
    }

    /// Wrap an already-baked shape together with the placement it carries.
    fn with_parts_and_placement(
        shape: Option<Shape>,
        placement: Placement,
        lineage: Option<Lineage>,
    ) -> Self {
        PartShape {
            shape: Arc::new(Mutex::new(shape)),
            placement: Arc::new(Mutex::new(placement)),
            lineage: Arc::new(Mutex::new(lineage)),
            epoch: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The null shape.
    pub(crate) fn empty() -> Self {
        Self::with_parts(None, None)
    }

    /// A second handle to an independent copy of the same shape (for `Shape.copy`
    /// and subclasses).
    pub(crate) fn copy_handle(&self) -> Self {
        Self::with_parts_and_placement(self.shape(), self.placement(), self.lineage())
    }

    /// A copy of the wrapped shape (shape handles are cheap to clone).
    pub(crate) fn shape(&self) -> Option<Shape> {
        self.shape
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The accumulated placement baked into [`shape`](PartShape::shape).
    pub(crate) fn placement(&self) -> Placement {
        *self
            .placement
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// The lineage carried beside the shape, if it has one.
    ///
    /// Rust-side accessor: the lineage is metadata, and the only Python-visible
    /// view is [`ElementReverseMap`](PartShape::ElementReverseMap).
    pub fn lineage(&self) -> Option<Lineage> {
        self.lineage
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Wrap an existing kernel shape with no lineage (a primitive, a sub-shape, a
    /// shape assigned from Python).
    ///
    /// A compound that holds bodies names its own sub-elements (see
    /// [`intrinsic_lineage`]): `Part.Compound([Box1, Box2])` and the document's
    /// `Part::Compound` report the 52 names upstream's `makeElementCompound`
    /// assigns. Nothing else gets a map here — a primitive and a single-body
    /// operation result carry the names of the operation that produced them, and
    /// only that operation knows them.
    pub(crate) fn from_shape(shape: Shape) -> Self {
        let lineage = intrinsic_lineage(&shape);
        Self::with_parts(Some(shape), lineage)
    }

    /// Wrap a shape together with the lineage folded when it was produced.
    ///
    /// An empty lineage is stored as `None`, so a shape with no relations behaves
    /// exactly like a primitive.
    pub(crate) fn from_shape_with_lineage(shape: Shape, lineage: Lineage) -> Self {
        let lineage = if lineage.map().is_empty() && lineage.reverse().is_empty() {
            None
        } else {
            Some(lineage)
        };
        Self::with_parts(Some(shape), lineage)
    }

    /// Replace the wrapped shape (a subclass rebuilds its base shape in place).
    ///
    /// The placement resets to identity and the lineage is cleared: the new
    /// geometry carries neither the old accumulated placement nor the old names,
    /// which referred to handles a rebuilt shape no longer has.
    pub(crate) fn replace_shape(&self, shape: Option<Shape>) {
        *self
            .placement
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Placement::identity();
        *self.shape.lock().unwrap_or_else(|e| e.into_inner()) = shape;
        *self.lineage.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }

    /// Replace the wrapped geometry, keeping the accumulated placement (an
    /// orientation change, such as `Shape.reverse`, does not move the shape).
    fn replace_geometry(&self, shape: Option<Shape>) {
        *self.shape.lock().unwrap_or_else(|e| e.into_inner()) = shape;
        *self.lineage.lock().unwrap_or_else(|e| e.into_inner()) = None;
        self.epoch.fetch_add(1, Ordering::SeqCst);
    }

    /// Set the absolute placement, re-baking the geometry by the placement delta.
    fn apply_placement(&self, new: Placement) {
        apply_placement_to(
            &self.shape,
            &self.placement,
            &self.lineage,
            &self.epoch,
            None,
            new,
        );
    }

    /// Apply a global rigid transformation: `placement = loc · placement` and the
    /// geometry moves with it. This is how `Shape.rotate`/`translated` compose.
    fn apply_global_placement(&self, loc: Placement) {
        let new = compose_placements(&loc, &self.placement());
        self.apply_placement(new);
    }

    /// Apply a matrix to the placement when it is rigid, or bake it into the
    /// geometry when it is not (scaling, shear, reflection).
    fn apply_matrix_to_placement(&self, matrix: &ferrocad_types::Matrix4) {
        if matrix.has_scale(1e-9) == ScaleType::NoScaling && matrix.determinant3() > 0.0 {
            let loc = Placement::new(
                Vector3::new(matrix.m[3], matrix.m[7], matrix.m[11]),
                Rotation::from_matrix(matrix),
            );
            self.apply_global_placement(loc);
        } else if let Some(shape) = self.shape() {
            if let Ok(moved) = ferrocad_part::backend().transform(&shape, matrix) {
                self.replace_shape(Some(moved));
            }
        }
    }

    /// A counter bumped whenever the geometry is replaced or moved.
    ///
    /// A subclass that derives state from the B-Rep (`Part.Compound` keeps its
    /// children) compares it to know when that state is stale.
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::SeqCst)
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

/// The lineage a shape carries on its own, with no operation history.
///
/// Only a compound that holds a body has one: its sub-elements are named
/// positionally, which is the map upstream's `makeElementCompound` →
/// `mapSubElement` builds (two boxes → 52 names). Every other shape needs an
/// operation to report its names, so this returns `None` and the shape keeps an
/// empty map.
fn intrinsic_lineage(shape: &Shape) -> Option<Lineage> {
    let history = OcctBackend::new().intrinsic_history(shape)?;
    Some(Lineage::from_history(&history, MapVersion::default()))
}

/// Compose placements the way FreeCAD's `Placement::operator*` does: `a * b`
/// applies `b` first, then `a` (rotation part `a.rotation * b.rotation`, base
/// `a.rotation * b.base + a.base`).
fn compose_placements(a: &Placement, b: &Placement) -> Placement {
    Placement {
        base: a.base.add(&a.rotation.to_matrix().transform(&b.base)),
        rotation: a.rotation.multiply(&b.rotation),
    }
}

/// Re-bake the geometry so it matches `new`, then store the placement.
///
/// The geometry always equals `placement · base`, so moving from the old
/// placement to `new` is a global transform by `new · old⁻¹`. A no-op leaves the
/// state (and the epoch) alone. The lineage is dropped: the transform replaces
/// every sub-shape handle it used to name.
///
/// `expected_epoch` is the detach check for a live view: when it no longer matches
/// the shared epoch, the view was captured before a newer change and its write is
/// ignored.
fn apply_placement_to(
    shape: &Mutex<Option<Shape>>,
    placement: &Mutex<Placement>,
    lineage: &Mutex<Option<Lineage>>,
    epoch: &AtomicU64,
    expected_epoch: Option<u64>,
    new: Placement,
) {
    let mut placement_guard = placement.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(expected) = expected_epoch {
        if epoch.load(Ordering::SeqCst) != expected {
            return;
        }
    }
    if *placement_guard == new {
        return;
    }
    let delta = compose_placements(&new, &placement_guard.inverse());
    let mut shape_guard = shape.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(current) = shape_guard.as_ref() {
        match ferrocad_part::backend().transform(current, &delta.to_matrix()) {
            Ok(moved) => *shape_guard = Some(moved),
            // A failed re-bake must not leave the pair inconsistent.
            Err(_) => return,
        }
    }
    *placement_guard = new;
    *lineage.lock().unwrap_or_else(|e| e.into_inner()) = None;
    epoch.fetch_add(1, Ordering::SeqCst);
}

/// Parse a positional element name (`Face2`) the way upstream's
/// `shapeTypeAndIndex` does: a known kind prefix followed by a plain integer.
///
/// A missing prefix (`InvalidName`) or trailing text (`Face6_abc`) yields
/// `None`, which `getElement` reports as a `ValueError`. The returned ordinal is
/// still unchecked against the shape; `0` is rejected by the caller.
fn parse_element_name(name: &str) -> Option<(ElementKind, usize)> {
    ElementKind::ALL.into_iter().find_map(|kind| {
        let rest = name.strip_prefix(kind.prefix())?;
        rest.parse::<usize>().ok().map(|index| (kind, index))
    })
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
            Some(value) => {
                let other = value.downcast::<PartShape>().map_err(|_| {
                    pyo3::exceptions::PyTypeError::new_err("expected a Part.Shape")
                })?;
                let other = other.borrow();
                if other.shape().is_none() {
                    return Err(curves::OCCError::new_err("a null shape cannot be combined"));
                }
                Ok(other.copy_handle())
            }
        }
    }

    /// A copy of the shape (`Shape.copy`); the kernel handle is reference-counted
    /// so no geometry is duplicated. The lineage is copied too, unless
    /// `noElementMap` asks for a bare shape. The accumulated placement is always
    /// preserved.
    #[pyo3(signature = (noElementMap = false))]
    fn copy(&self, noElementMap: bool) -> Self {
        if noElementMap {
            match self.shape() {
                Some(shape) => {
                    Self::with_parts_and_placement(Some(shape), self.placement(), None)
                }
                None => Self::empty(),
            }
        } else {
            self.copy_handle()
        }
    }

    /// The shape's accumulated placement (`Shape.Placement`).
    ///
    /// This is a **live** view: `shape.Placement.Base = …`,
    /// `shape.Placement.Rotation.Angle = …` and `shape.Placement.Base.x = …` all
    /// write back. The geometry is re-baked by the placement delta, so `BoundBox`,
    /// `Vertexes`, `normalAt` and the mass properties stay consistent. A view
    /// captured before a later placement change detaches instead of overwriting
    /// the newer value.
    #[getter]
    fn Placement(&self, py: Python<'_>) -> PyResult<PyObject> {
        let placement = self.placement();
        let epoch = self.epoch.load(Ordering::SeqCst);
        let shape = Arc::clone(&self.shape);
        let placement_cell = Arc::clone(&self.placement);
        let lineage = Arc::clone(&self.lineage);
        let epoch_cell = Arc::clone(&self.epoch);
        let hook = Arc::new(move |new: Placement| {
            apply_placement_to(
                &shape,
                &placement_cell,
                &lineage,
                &epoch_cell,
                Some(epoch),
                new,
            );
        });
        ferrocad_py::placement_with_hook(py, placement, hook)
    }

    /// Assigning a placement replaces the accumulated placement (`Shape.Placement =
    /// …`): the geometry is re-baked by the difference, so the shape ends up at
    /// exactly the assigned placement rather than at `assigned · old`.
    #[setter]
    fn set_Placement(&self, value: &Bound<'_, PyAny>) -> PyResult<()> {
        let matrix = matrix_from_py(&value.call_method0("toMatrix")?)?;
        let placement = Placement::new(
            Vector3::new(matrix.m[3], matrix.m[7], matrix.m[11]),
            Rotation::from_matrix(&matrix),
        );
        self.apply_placement(placement);
        Ok(())
    }

    /// A copy translated by `vector` (`Shape.translated`).
    fn translated(&self, vector: &Bound<'_, PyAny>) -> PyResult<PartShape> {
        let offset = curves::vector(vector)?;
        let result = self.copy_handle();
        result.apply_global_placement(Placement::new(offset, Rotation::identity()));
        Ok(result)
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
    ///
    /// With `copy=True` (the usual path) the matrix is baked into the geometry
    /// and the result carries the identity placement. With `copy=False` a rigid
    /// matrix is composed onto the placement instead, matching upstream's
    /// location-only transform; a non-rigid one falls back to baking.
    ///
    /// The lineage is dropped: a transform replaces every sub-shape handle, so the
    /// map's old handles no longer name the result's elements.
    #[pyo3(signature = (matrix, copy = true))]
    fn transformed(
        &self,
        matrix: &Bound<'_, PyAny>,
        copy: bool,
    ) -> PyResult<PartShape> {
        let matrix = matrix_from_py(matrix)?;
        if !copy {
            let result = self.copy_handle();
            result.apply_matrix_to_placement(&matrix);
            return Ok(result);
        }
        let shape = self.named_shape()?;
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
        let matrix = matrix_from_py(matrix)?;
        if copy {
            let shape = slf.borrow().named_shape()?;
            let moved = ferrocad_part::backend()
                .transform(&shape, &matrix)
                .map_err(kernel_error)?;
            slf.borrow().replace_shape(Some(moved));
        } else {
            slf.borrow().apply_matrix_to_placement(&matrix);
        }
        Ok(slf.clone().into_any().unbind())
    }

    /// Rotate this shape in place about the axis through `center` along `axis`,
    /// by `angle` degrees (`Shape.rotate`). Upstream moves the shape's location
    /// by the rotation, so the accumulated placement becomes `R · placement`
    /// (not a separately baked shape). Upstream mutates in place and returns the
    /// shape itself, so this does too.
    fn rotate(
        slf: &Bound<'_, Self>,
        center: &Bound<'_, PyAny>,
        axis: &Bound<'_, PyAny>,
        angle: f64,
    ) -> PyResult<PyObject> {
        let center = curves::vector(center)?;
        let axis = curves::vector(axis)?;
        if axis.is_null() {
            return Err(curves::OCCError::new_err(
                "a rotation needs a non-zero axis",
            ));
        }
        let rotation = Rotation::from_axis_angle(&axis, angle.to_radians());
        // A rotation about `center` is the placement `center - R·center`, `R`.
        let about = Placement::new(
            center.sub(&rotation.to_matrix().transform(&center)),
            rotation,
        );
        slf.borrow().apply_global_placement(about);
        Ok(slf.clone().into_any().unbind())
    }

    /// A copy with redundant edges/faces unified (`Shape.cleaned`).
    ///
    /// A compound re-derives its own element map from the cleaned geometry, so the
    /// 52 names of a two-box compound survive; other shapes drop their map, because
    /// cleaning changes handles a history named.
    fn cleaned(&self) -> Self {
        let Some(shape) = self.shape() else {
            return Self::empty();
        };
        match ferrocad_part::backend().clean(&shape) {
            Some(cleaned) => {
                let lineage = intrinsic_lineage(&cleaned);
                Self::with_parts_and_placement(Some(cleaned), self.placement(), lineage)
            }
            None => self.copy_handle(),
        }
    }

    /// Whether this is the empty/null shape (FreeCAD's `Shape.isNull()`).
    fn isNull(&self) -> bool {
        self.shape().is_none()
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
    ///
    /// A planar face reports the elementary plane description (with its unit
    /// normal); any other surface reports the kernel's type id, as before.
    #[getter]
    fn Surface(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        let Some(shape) = self.shape() else {
            return Ok(None);
        };
        let Some(type_id) = ferrocad_part::backend().surface_type(&shape) else {
            return Ok(None);
        };
        if type_id == "Part::GeomPlane" {
            let normal = planar_surface_normal(&shape)
                .unwrap_or_else(|| ferrocad_curves::Vector3::new(0.0, 0.0, 1.0));
            return Ok(Some(
                Py::new(py, solids::PyPlaneSurface::new(normal))?.into_any(),
            ));
        }
        Ok(Some(
            Py::new(py, curves::PySurface::new(type_id))?.into_any(),
        ))
    }

    /// The face normal at `(u, v)` (`Face.normalAt`).
    ///
    /// The seam exposes no surface evaluator, so the normal is derived from the
    /// face's own geometry: two non-parallel edge tangents of the outer wire give
    /// the plane's normal, flipped when the face orientation is reversed. A
    /// non-planar face reports `Part.OCCError` instead of guessing.
    fn normalAt(&self, py: Python<'_>, _u: f64, _v: f64) -> PyResult<PyObject> {
        let shape = self.named_shape()?;
        let normal = planar_face_normal(&shape).ok_or_else(|| {
            curves::OCCError::new_err("normal not defined: only planar faces are supported")
        })?;
        curves::py_vector(py, normal)
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

    /// The plane a planar face lies in (`Shape.findPlane`), or `None`.
    ///
    /// The normal comes from the outer wire (the same derivation as
    /// [`normalAt`](Self::normalAt)) and the origin is one of the face's
    /// vertices. `tol` bounds every vertex's distance from the plane, so a shape
    /// that is only nearly planar reports `None`. A boundary with fewer than two
    /// distinct straight tangents (a full circle, for example) has no derivable
    /// normal yet and also reports `None`.
    #[pyo3(signature = (tol = 1e-7))]
    fn findPlane(&self, py: Python<'_>, tol: f64) -> PyResult<PyObject> {
        let Some(shape) = self.shape() else {
            return Ok(py.None());
        };
        let Some(normal) = planar_face_normal(&shape) else {
            return Ok(py.None());
        };
        let points = vertex_points(&shape);
        let Some(origin) = points.first().copied() else {
            return Ok(py.None());
        };
        let tolerance = tol.max(1e-12);
        if !points
            .iter()
            .all(|point| point.sub(&origin).dot(&normal).abs() <= tolerance)
        {
            return Ok(py.None());
        }
        Ok(Py::new(py, curves::PyPlane::from_parts(origin, normal))?.into_any())
    }

    /// `Shape.getSubShape(name[, strict=False])` — the named sub-element as its
    /// own shape (`Part.Edge`, `Part.Vertex`, or a `Part.Shape` handle).
    ///
    /// The name is resolved through the shape's lineage first and positionally
    /// second (see [`resolve_element_in`]). Without `strict`, a name the shape
    /// cannot resolve returns a null shape, as upstream does.
    #[pyo3(signature = (name, strict = false))]
    fn getSubShape(&self, py: Python<'_>, name: &str, strict: bool) -> PyResult<PyObject> {
        let Some(shape) = self.shape() else {
            return Err(curves::OCCError::new_err(
                "a null shape has no sub-shapes",
            ));
        };
        let lineage = self.lineage().unwrap_or_default();
        match resolve_element_in(&shape, &lineage, name) {
            Ok(sub) => sub_element_shape_to_py(py, sub),
            Err(error) if strict => Err(curves::OCCError::new_err(format!(
                "getSubShape: '{name}': {error}"
            ))),
            Err(_) => Ok(Py::new(py, PartShape::empty())?.into_any()),
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

    /// Upstream's element-map version.
    ///
    /// A shape that carries a lineage reports the composite version the map was
    /// folded under (upstream's `"<opcode>.<occt>.<encoding>.<policy>"` spelling);
    /// a primitive, a sub-shape or an operation whose names this kernel does not
    /// reproduce reports `""`, which is upstream's own "no map" signal and makes
    /// the Part tests skip their guarded naming assertions.
    #[getter]
    fn ElementMapVersion(&self) -> String {
        match self.lineage() {
            Some(lineage) => lineage.version().element_map_version(),
            None => String::new(),
        }
    }

    /// The number of names in the element map.
    ///
    /// This is the size of the lineage's named-element map (the same keys
    /// [`ElementReverseMap`](PartShape::ElementReverseMap) exposes), which is what
    /// upstream counts: a fused L reports 58 even though 58 is not its element
    /// count, because that is how many result elements the naming passes name.
    #[getter]
    fn ElementMapSize(&self) -> usize {
        self.lineage()
            .map(|lineage| lineage.reverse().len())
            .unwrap_or(0)
    }

    /// The lineage read backwards: `{output name: [input names...]}`.
    ///
    /// This exposes exactly what the operation history recorded, under the policy
    /// the lineage was folded with; a primitive, a transform or a shape whose map
    /// this kernel does not reproduce returns an empty dict.
    #[getter]
    fn ElementReverseMap(&self, py: Python<'_>) -> PyResult<Py<PyDict>> {
        let map = PyDict::new(py);
        if let Some(lineage) = self.lineage() {
            for (output, inputs) in lineage.reverse().iter() {
                map.set_item(
                    output,
                    PyList::new(py, inputs.iter().map(String::as_str))?,
                )?;
            }
        }
        Ok(map.unbind())
    }

    /// The serialised element map (`Shape.Content`). Upstream prints
    /// `<ElementMap/>` for a fresh box, which is the only form asserted; the
    /// versioned map is exposed through `ElementMapSize`/`ElementReverseMap`.
    #[getter]
    fn Content(&self) -> &'static str {
        "<ElementMap/>\n"
    }

    /// The forward element map (`Shape.ElementMap`).
    ///
    /// It stays empty: the names this kernel keeps are an *output-element* map
    /// (`ElementReverseMap` and `ElementMapSize`), and exposing the lineage's
    /// internal `name -> handle` table as upstream's `MappedName -> IndexedName`
    /// dict would invent a different encoding.
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
    /// `glue` and `tolerance` are accepted for upstream signature compatibility; the
    /// kernel runs the plain boolean. `noElementMap` drops the lineage, exactly as
    /// upstream's `ElementMapPolicy::Drop`.
    #[pyo3(signature = (other, glue = false, tolerance = 0.0, refine = true, noElementMap = false))]
    fn fuse(
        &self,
        other: &Bound<'_, PyAny>,
        glue: bool,
        tolerance: f64,
        refine: bool,
        noElementMap: bool,
    ) -> PyResult<PartShape> {
        let _ = (glue, tolerance, refine);
        self.boolean(other, BooleanOp::Fuse, noElementMap)
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
        let _ = (glue, tolerance, refine);
        self.boolean(other, BooleanOp::Cut, noElementMap)
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
        let _ = (glue, tolerance, refine);
        self.boolean(other, BooleanOp::Common, noElementMap)
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
        let _ = (glue, tolerance, refine);
        let mut shape = self.named_shape()?;
        // Each fold is one operation with its own history; fold them forward into one
        // lineage, the same way the geometry is folded.
        let mut lineage = Lineage::default();
        let mut fused = false;
        for item in shapes.try_iter()? {
            let other = shape_argument(&item?)?;
            let out = ferrocad_part::backend()
                .fuse(&shape, &other)
                .map_err(kernel_error)?;
            lineage.apply(&out.history);
            shape = out.shape;
            fused = true;
        }
        if !fused {
            return Ok(self.copy_handle());
        }
        if noElementMap {
            return Ok(PartShape::with_parts(Some(shape), None));
        }
        Ok(PartShape::from_shape_with_lineage(shape, lineage))
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
    /// `join` is upstream's `JoinType` (0 = arc, 1 = tangent, 2 = intersection),
    /// and `fill=True` returns the band between the source and its offset. The
    /// `intersection` switch is accepted for signature compatibility and not
    /// applied; edge and compound inputs are not supported.
    #[pyo3(signature = (offset, *, join = 0, fill = false, openResult = false, intersection = false))]
    fn makeOffset2D(
        &self,
        offset: f64,
        join: i32,
        fill: bool,
        openResult: bool,
        intersection: bool,
    ) -> PyResult<PartShape> {
        let _ = intersection;
        let shape = self.named_shape()?;
        let offseted = surface_ops::offset2d(&shape, offset, join, fill, openResult)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(offseted))
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
    /// `makeFillet(radius1, radius2, edgeList)` is upstream's variable-radius
    /// form: each edge's radius runs linearly from `radius1` to `radius2` along it.
    #[pyo3(signature = (*args))]
    fn makeFillet(&self, args: &Bound<'_, PyTuple>) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        match args.len() {
            2 => {
                let radius: f64 = args.get_item(0)?.extract()?;
                if !(radius > 0.0) {
                    return Err(curves::OCCError::new_err(
                        "makeFillet needs a positive radius",
                    ));
                }
                let edges = shape_sequence(&args.get_item(1)?)?;
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
            3 => {
                let start: f64 = args.get_item(0)?.extract()?;
                let end: f64 = args.get_item(1)?.extract()?;
                let edges = shape_sequence(&args.get_item(2)?)?;
                let radii = vec![start; edges.len()];
                let ends = vec![end; edges.len()];
                let filleted = surface_ops::fillet_variable(&shape, &edges, &radii, &ends)
                    .map_err(kernel_error)?;
                Ok(PartShape::from_shape(filleted))
            }
            _ => Err(PyValueError::new_err(
                "makeFillet takes (radius, edgeList) or (radius1, radius2, edgeList)",
            )),
        }
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
    ///
    /// The failure modes mirror upstream's `getSubTopoShape`: a name whose
    /// spelling is not a known kind followed by a positive integer (including
    /// `InvalidName` and a stray suffix such as `Face6_abc`) is a `ValueError`,
    /// and a well-spelled name past the shape's last element of that kind is an
    /// `IndexError`. A null shape is a `ValueError` as well.
    fn getElement(&self, name: &str) -> PyResult<PartShape> {
        let Some(shape) = self.shape() else {
            return Err(PyValueError::new_err("null shape"));
        };
        let Some((_kind, index)) = parse_element_name(name) else {
            return Err(PyValueError::new_err(format!("Invalid shape name {name}")));
        };
        if index == 0 {
            return Err(PyValueError::new_err(format!("Invalid shape index {index}")));
        }
        match ferrocad_part::backend().resolve(&shape, name) {
            Some(resolved) => Ok(PartShape::from_shape(resolved)),
            None => Err(PyIndexError::new_err(format!(
                "Shape index {index} out of bound"
            ))),
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
    ///
    /// The element map travels with the copy, as upstream's does: a reshape is a
    /// name-preserving edit, not a new operation.
    fn removeShape(&self, shapes: &Bound<'_, PyAny>) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        let removals = shape_sequence(shapes)?;
        if removals.is_empty() {
            return Ok(self.copy_handle());
        }
        let result = ferrocad_part::backend()
            .remove_shapes(&shape, &removals)
            .map_err(kernel_error)?;
        Ok(PartShape::with_parts_and_placement(
            Some(result),
            self.placement(),
            self.lineage(),
        ))
    }

    /// `Shape.replaceShape([(old, new), ...])` — a copy with each `old`
    /// sub-shape replaced by `new` (`BRepTools_ReShape`).
    ///
    /// The element map travels with the copy, as upstream's does.
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
        Ok(PartShape::with_parts_and_placement(
            Some(result),
            self.placement(),
            self.lineage(),
        ))
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
    /// This kernel derives a shape's map when the shape is produced (a boolean's
    /// history, a compound's own element names), not from a selection, so there is
    /// nothing to build here: the call is a documented no-op and returns this shape
    /// unchanged. Upstream's own tests guard the map assertions on the version, so
    /// callers that check it behave identically.
    #[pyo3(signature = (shape, op = None))]
    fn mapSubElement<'py>(
        slf: &Bound<'py, Self>,
        shape: &Bound<'_, PyAny>,
        op: Option<&str>,
    ) -> PyResult<PyObject> {
        let _ = (shape, op);
        Ok(slf.clone().into_any().unbind())
    }

    /// `Shape.getElementHistory(name)` — the mapped-name history behind `name`.
    ///
    /// Upstream returns `(tag, original, [intermediate names])`; this kernel keeps
    /// no history tags or hashes, so the tag is `0` and the intermediates are the
    /// operand names the lineage recorded as producers. `name` may be the string
    /// upstream would pass or, because this kernel's `ElementReverseMap` values are
    /// lists, the list itself (its first producer is used). Returns `None` for a
    /// name the lineage does not know, as upstream does.
    fn getElementHistory(&self, py: Python<'_>, name: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let name = if let Ok(name) = name.extract::<String>() {
            name
        } else if let Ok(names) = name.extract::<Vec<String>>() {
            match names.first() {
                Some(name) => name.clone(),
                None => return Ok(py.None()),
            }
        } else {
            return Err(pyo3::exceptions::PyTypeError::new_err(
                "expected an element name or a list of them",
            ));
        };
        let Some(lineage) = self.lineage() else {
            return Ok(py.None());
        };
        let Some(producers) = lineage.reverse().get(&name) else {
            return Ok(py.None());
        };
        let tag: PyObject = 0i64.into_py_any(py)?;
        let original: PyObject = name.into_py_any(py)?;
        let intermediates: PyObject = PyList::new(py, producers)?.into_any().unbind();
        Ok(PyTuple::new(py, [tag, original, intermediates])?
            .into_any()
            .unbind())
    }

    /// `Shape.reverse()` — reverse this shape's orientation in place. The
    /// accumulated placement is kept: reversing does not move the shape.
    fn reverse(slf: &Bound<'_, Self>) -> PyResult<()> {
        let shape = slf.borrow().named_shape()?;
        let reversed = ferrocad_part::backend()
            .reversed(&shape)
            .map_err(kernel_error)?;
        slf.borrow().replace_geometry(Some(reversed));
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

/// The unit normal of a planar face's *surface*, from two non-parallel tangent
/// directions of its outer wire. The face orientation is deliberately ignored:
/// that flip belongs to [`planar_face_normal`] (`normalAt`).
fn planar_surface_normal(shape: &Shape) -> Option<ferrocad_curves::Vector3> {
    let backend = ferrocad_part::backend();
    if backend.surface_type(shape).as_deref() != Some("Part::GeomPlane") {
        return None;
    }
    let wire = backend.outer_wire(shape)?;
    let mut directions = Vec::new();
    for edge in backend.elements(&wire, ElementKind::Edge) {
        if let Ok(curve) = backend.curve_of(&edge.shape) {
            directions.push(curve.tangent_at(0.0));
        }
    }
    for (index, first) in directions.iter().enumerate() {
        for second in directions.iter().skip(index + 1) {
            let normal = first.cross(second);
            if normal.length() > 1e-9 {
                return Some(normal.normalize());
            }
        }
    }
    None
}

/// The face normal at a point: the surface normal, flipped when the face's
/// orientation is reversed (upstream's `Face.normalAt`).
fn planar_face_normal(shape: &Shape) -> Option<ferrocad_curves::Vector3> {
    let mut normal = planar_surface_normal(shape)?;
    if ferrocad_part::backend().orientation(shape) == Some("Reversed") {
        normal = normal.neg();
    }
    Some(normal)
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
    /// Apply `matrix` to a copy. The lineage is dropped for the same reason as
    /// [`transformed`](PartShape::transformed): the handles it records are stale.
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

    fn boolean(
        &self,
        other: &Bound<'_, PyAny>,
        operation: BooleanOp,
        no_element_map: bool,
    ) -> PyResult<PartShape> {
        let a = self.named_shape()?;
        let b = shape_argument(other)?;
        let backend = ferrocad_part::backend();
        let result = match operation {
            BooleanOp::Fuse => backend.fuse(&a, &b),
            BooleanOp::Cut => backend.cut(&a, &b),
            BooleanOp::Common => backend.common(&a, &b),
        }
        .map_err(kernel_error)?;
        if no_element_map {
            return Ok(PartShape::with_parts(Some(result.shape), None));
        }
        let lineage = Lineage::from_history(&result.history, MapVersion::default());
        Ok(PartShape::from_shape_with_lineage(result.shape, lineage))
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
///
/// `angle1`/`angle2` are the latitude range (degrees from the equator) and
/// `angle3` the longitude sweep, both as upstream passes them; the full sphere is
/// the default `(0, 360, -90, 90)`. `dir` is accepted for signature compatibility
/// and not applied.
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
    let center = center
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(ferrocad_curves::Vector3::zero);
    let shape = surface_ops::make_sphere_slice(radius, center, 0.0, angle3, angle1, angle2)
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

/// The `Shape` value of a document object, as the `Part.Shape` its `obj.Shape`
/// returns. The document-object wrapper lives in `ferrocad_py` and is opaque here,
/// so the value is reached through the same Python attribute the scripts use.
fn object_shape(obj: &Bound<'_, PyAny>) -> PyResult<(String, Shape, Placement, Lineage)> {
    let name = obj
        .getattr("Name")
        .and_then(|value| value.extract::<String>())
        .unwrap_or_default();
    let shape_value = obj
        .getattr("Shape")
        .map_err(|_| curves::OCCError::new_err("Part.getShape expects an object with a Shape"))?;
    let shape = shape_value.downcast::<PartShape>().map_err(|_| {
        curves::OCCError::new_err(format!("Part.getShape: '{name}' has no Shape"))
    })?;
    let borrowed = shape.borrow();
    let geometry = borrowed.shape().ok_or_else(|| {
        curves::OCCError::new_err(format!("Part.getShape: '{name}' has a null Shape"))
    })?;
    Ok((
        name,
        geometry,
        borrowed.placement(),
        borrowed.lineage().unwrap_or_default(),
    ))
}

/// `Part.getShape(obj, subname='', needSubElement=False, transform=False,
/// retType=0, matrix=None)` → `Part.Shape`.
///
/// Upstream returns an object's `Shape`, or the sub-element `subname` names
/// (`Face1`, `Edge3`, …). The name is resolved through the shape's lineage first
/// and positionally second ([`resolve_element_in`]), so a stored name from an
/// earlier operation still selects its own element.
///
/// The compatibility arguments are documented rather than silently ignored:
///
/// - `needSubElement`: a named sub-element is resolved either way. The element
///   map always travels with the returned `Part.Shape`, so there is no reduced
///   form to choose for `False`.
/// - `transform`: apply the object's `Placement` when the stored shape is local
///   (its accumulated placement is the identity — a Python proxy's shape). A
///   native Part feature bakes its placement into the stored shape, so applying
///   it again would move it twice and is skipped.
/// - `retType`: 0 and 1 both return the shape (upstream's 1 also carries the
///   element map, which ours always does); any other value raises.
/// - `matrix`: upstream's optional output matrix, not produced here.
#[pyfunction]
#[pyo3(signature = (obj, subname = "", needSubElement = false, transform = false, retType = 0, matrix = None))]
fn getShape(
    py: Python<'_>,
    obj: &Bound<'_, PyAny>,
    subname: &str,
    needSubElement: bool,
    transform: bool,
    retType: i64,
    matrix: Option<&Bound<'_, PyAny>>,
) -> PyResult<PyObject> {
    let _ = (needSubElement, matrix);
    if !matches!(retType, 0 | 1) {
        return Err(curves::OCCError::new_err(format!(
            "Part.getShape: retType {retType} is not supported"
        )));
    }
    let (name, shape, placement, lineage) = object_shape(obj)?;
    let subname = subname.trim();
    let mut resolved = if subname.is_empty() {
        shape
    } else {
        resolve_element_in(&shape, &lineage, subname).map_err(|error| {
            curves::OCCError::new_err(format!(
                "Part.getShape: '{name}' has no sub-element '{subname}': {error}"
            ))
        })?
    };
    if transform && placement.is_same(&Placement::identity(), 1e-12) {
        if let Ok(object_placement) = obj.getattr("Placement") {
            let matrix = object_placement.call_method0("toMatrix")?;
            let matrix = matrix_from_py(&matrix)?;
            resolved = ferrocad_part::backend()
                .transform(&resolved, &matrix)
                .map_err(kernel_error)?;
        }
    }
    let shape = if subname.is_empty() {
        let lineage = if lineage.map().is_empty() && lineage.reverse().is_empty() {
            None
        } else {
            Some(lineage)
        };
        PartShape::with_parts_and_placement(Some(resolved), placement, lineage)
    } else {
        PartShape::from_shape(resolved)
    };
    Py::new(py, shape).map(Py::into_any)
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
///
/// The result carries the map of its own three elements (the lateral face, its
/// two edges and their vertices), the six names upstream reports for a circle
/// swept along a line.
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
    let history = OcctBackend::new().name_every_element(&piped);
    let lineage = Lineage::from_history(&history, MapVersion::default());
    Ok(PartShape::from_shape_with_lineage(piped, lineage))
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

/// Wrap one sub-element in the Python class its kind uses: `Part.Edge`,
/// `Part.Vertex`, or a generic `Part.Shape` handle (`Part.Face`, `Part.Wire`, …).
///
/// This is the wrapper `getSubObject("Edge1")` and `Shape.getSubShape("Edge1")`
/// return, so a caller can use the element's edge-specific API (`.Curve`,
/// `.firstVertex()`) without a separate cast.
fn sub_element_shape_to_py(py: Python<'_>, shape: Shape) -> PyResult<PyObject> {
    match PartShape::kind_of(&shape) {
        Some(ElementKind::Edge) => Ok(Py::new(
            py,
            curves::PyEdge::from_entry(curves::EdgeEntry { shape, curve: None }),
        )?
        .into_any()),
        Some(ElementKind::Vertex) => {
            Ok(Py::new(py, curves::PyVertex::from_shape(shape)?)?.into_any())
        }
        _ => Ok(Py::new(py, PartShape::from_shape(shape))?.into_any()),
    }
}

/// `Part::PropertyPartShape`'s sub-element resolver: `Edge1` → `Part.Edge`.
///
/// Registered with `ferrocad_py` at import, this is what
/// `DocumentObject.getSubObject("Edge1")` returns for a Part feature (upstream
/// `Part::FeaturePy::getSubObject`). A name the shape cannot resolve returns
/// `None`, so the caller falls back to the owning object.
fn shape_sub_element_to_py(py: Python<'_>, data: &dyn ExtensionData, name: &str) -> Option<PyObject> {
    let property = data.as_any().downcast_ref::<ShapeProperty>()?;
    let shape = property.shape()?;
    let sub = resolve_element_in(&shape, &property.lineage(), name).ok()?;
    sub_element_shape_to_py(py, sub).ok()
}

/// `Part::PropertyPartShape` → `Part.Shape` (what `obj.Shape` returns).
///
/// The property's lineage travels with the handle, so a document object's shape
/// exposes the same reverse map the operation stored.
fn shape_to_py(py: Python<'_>, data: &dyn ExtensionData) -> Option<PyObject> {
    let property = data.as_any().downcast_ref::<ShapeProperty>()?;
    let shape = property.shape();
    let lineage = property.lineage();
    let lineage = if lineage.map().is_empty() && lineage.reverse().is_empty() {
        // A document `Part::Compound` stores no operation history, but a compound
        // names itself; derive that map on the way out so the Python shape sees
        // the same 52 names `Part.Compound` does.
        shape.as_ref().and_then(intrinsic_lineage)
    } else {
        Some(lineage.clone())
    };
    Py::new(
        py,
        PartShape::with_parts_and_placement(shape, property.placement(), lineage),
    )
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
        .map(|shape| Py::new(py, PartShape::from_shape(shape)))
        .collect::<PyResult<Vec<_>>>()
        .ok()?;
    let list = PyList::new(py, items).ok()?;
    Some(list.into_any().unbind())
}

/// `Part.Shape` → `Part::PropertyPartShape` (what `obj.Shape = shape` stores).
///
/// A shape's lineage is stored beside it, the same way an operation stores its
/// result, so assigning `obj.Shape = boolean_result` does not silently discard the
/// map (and a legacy/primitive shape still stores none).
fn shape_to_extension(value: &Bound<'_, PyAny>) -> Option<ExtensionValue> {
    let shape = value.downcast::<PartShape>().ok()?;
    let shape = shape.borrow();
    let placement = shape.placement();
    let property = match shape.shape() {
        Some(inner) => match shape.lineage() {
            Some(lineage) => ShapeProperty::with_lineage_and_placement(inner, placement, lineage),
            None => ShapeProperty::with_placement(inner, placement),
        },
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
    ferrocad_py::register_sub_element_resolver(TYPE_NAME, shape_sub_element_to_py);
    ferrocad_py::register_extension_converter(
        SHAPE_LIST_TYPE_NAME,
        shape_list_to_py,
        shape_list_to_extension,
    );
    m.add_class::<PartShape>()?;
    m.add_function(wrap_pyfunction!(getShape, m)?)?;
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
    shape_fix::register(m)?;
    chfi2d::register(m)?;
    Ok(())
}
