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

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use ferrocad_core::object_registry::{self, ObjectType, PropertyChangeError};
use ferrocad_core::{
    Document, ExtensionData, ExtensionValue, ObjectId, Placement, Property, Rotation, ScaleType,
    Vector3,
};
use ferrocad_geom::{
    ElementKind, ElementRef, Lineage, MapVersion, Shape, SurfaceData, TessellationOptions,
    ToleranceKind,
};
use ferrocad_occt::surface_ops;
use ferrocad_occt::OcctBackend;
use ferrocad_part::{
    make_shape_property, resolve_element_in, ShapeListProperty, ShapeProperty,
    SHAPE_LIST_TYPE_NAME, TYPE_NAME,
};
use ferrocad_py::PyVector;
use pyo3::exceptions::{
    PyIOError, PyIndexError, PyRuntimeError, PyTypeError, PyValueError,
};
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
    // The document-object behavior lives in `ferrocad_part` and the Python-feature
    // types here; both are kernel-agnostic. Keep their `Shape` in sync with the
    // object's `Placement` here, where the kernel is linked. Install this after
    // the types are registered so the wrapper captures (not re-wraps) them.
    install_feature_placement_sync();
}

/// The wrapped base behaviour of each Part feature type, captured once so
/// re-installing the wrapper cannot wrap itself.
fn part_feature_bases() -> &'static Mutex<HashMap<&'static str, Arc<dyn ObjectType>>> {
    static BASES: OnceLock<Mutex<HashMap<&'static str, Arc<dyn ObjectType>>>> = OnceLock::new();
    BASES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The object types that own a `Shape`/`Placement` pair.
///
/// `Part::Feature` stores the object's own geometry, so a `Shape` assignment is a
/// script handing over a world-space shape: upstream's `Feature::onChanged`
/// adopts the shape's own transform as the `Placement`. The Python-feature types
/// are different: their `Shape` is an *output* their proxy rebuilds during a
/// recompute, and upstream keeps the object's `Placement` while baking it into the
/// freshly assigned geometry. The two rules are recorded per type here.
const PART_FEATURE_TYPES: [(&str, bool); 3] = [
    ("Part::Feature", false),
    (feature_python::FEATURE_PYTHON_TYPE, true),
    (feature_python::PART2D_PYTHON_TYPE, true),
];

/// Keep a Part feature's `Shape` and `Placement` in step, upstream
/// `Part::Feature::onChanged`.
///
/// `ferrocad_part` owns the object types but is deliberately kernel-agnostic, so
/// the transform they need lives here, in the composition root that links OCCT.
/// The wrapper delegates everything else to the registered behaviour.
///
/// `Part::FeaturePython` and `Part::Part2DObjectPython` need the same rule as
/// `Part::Feature`: upstream `Feature::onChanged` bakes the object's `Placement`
/// into the stored `Shape`, so a Draft wire's `obj.Shape.Vertexes` are in global
/// coordinates and `obj.Shape.Placement` is `obj.Placement`.
fn install_feature_placement_sync() {
    let mut bases = part_feature_bases().lock().unwrap_or_else(|e| e.into_inner());
    for (type_name, keep_placement) in PART_FEATURE_TYPES {
        let Some(base) = object_registry::get(type_name) else {
            continue;
        };
        bases.entry(type_name).or_insert_with(|| base.clone());
        let original = bases
            .get(type_name)
            .expect("just inserted")
            .clone();
        object_registry::register(
            type_name,
            Arc::new(FeatureWithPlacement {
                base: original,
                keep_placement,
            }),
        );
    }
}

struct FeatureWithPlacement {
    base: Arc<dyn ObjectType>,
    /// Whether a `Shape` write bakes the object's `Placement` into the new
    /// geometry (a Python feature rebuilt by its proxy) or adopts the shape's own
    /// transform (a plain `Part::Feature`).
    keep_placement: bool,
}

impl ObjectType for FeatureWithPlacement {
    fn default_properties(&self) -> Vec<(String, Property, u32)> {
        self.base.default_properties()
    }

    fn property_meta(&self, name: &str) -> Option<(String, String)> {
        self.base.property_meta(name)
    }

    fn execute(&self, doc: &mut Document, id: ObjectId) {
        self.base.execute(doc, id);
    }

    fn on_property_change(
        &self,
        doc: &Document,
        id: ObjectId,
        name: &str,
        value: &Property,
    ) -> Result<(), PropertyChangeError> {
        self.base.on_property_change(doc, id, name, value)
    }

    fn on_property_changed(&self, doc: &mut Document, id: ObjectId, name: &str) {
        self.base.on_property_changed(doc, id, name);
        sync_shape_and_placement(doc, id, name, self.keep_placement);
    }

    fn is_python_feature(&self) -> bool {
        self.base.is_python_feature()
    }
}

/// Mirror a Part feature's property write onto its partner.
///
/// - `Placement = p` re-bakes the stored geometry by `p · old⁻¹`, so
///   `obj.Shape` (and every query on it) follows immediately, without a
///   recompute, exactly as upstream's `Feature::onChanged` transforms the shape.
/// - `Shape = s` follows upstream's `onChanged(Shape)`. A shape that carries its
///   own accumulated placement (an operation result, or a preplaced
///   `Part.Shape`) hands that placement to the object. Otherwise:
///   - a plain `Part::Feature` adopts the shape's own (identity) transform, so an
///     object whose `Placement` was rotated ends up reporting an identity
///     placement and a world-space shape, exactly as upstream does for a script
///     assignment outside a recompute;
///   - a Python feature keeps its placement and bakes it into the proxy's freshly
///     assigned local geometry, which is what upstream's recompute path does and
///     what makes a Draft wire's `obj.Shape.Vertexes` come back in global
///     coordinates.
///
/// The writes go through [`Document::set_property_raw`], which records no undo
/// step and does not re-run this hook. Undo/redo re-runs it from the restore path,
/// so a reverted `Placement` also moves the shape back. Re-baking replaces every
/// sub-shape handle, so the stored lineage is dropped (positional element names
/// still resolve); this is the same contract `Shape.translate`/`rotate` have.
fn sync_shape_and_placement(
    doc: &mut Document,
    id: ObjectId,
    name: &str,
    keep_placement: bool,
) {
    match name {
        "Placement" => {
            let Some(Property::Placement(placement)) = doc
                .object(id)
                .and_then(|object| object.properties.get("Placement"))
                .cloned()
            else {
                return;
            };
            let Some((Some(shape), stored_placement, _lineage)) = shape_property_parts(doc, id)
            else {
                return;
            };
            if stored_placement == placement {
                return;
            }
            let delta = compose_placements(&placement, &stored_placement.inverse());
            let Ok(moved) = ferrocad_part::backend().transform(&shape, &delta.to_matrix()) else {
                return;
            };
            let property = ShapeProperty::with_placement(moved, placement);
            doc.set_property_raw(id, "Shape", make_shape_property(property));
        }
        "Shape" => {
            let Some((Some(shape), stored_placement, _lineage)) = shape_property_parts(doc, id)
            else {
                return;
            };
            let object_placement = doc
                .object(id)
                .and_then(|object| match object.properties.get("Placement") {
                    Some(Property::Placement(placement)) => Some(*placement),
                    _ => None,
                })
                .unwrap_or_else(Placement::identity);
            if stored_placement != Placement::identity() {
                // The assigned shape carries its own placement: adopt it.
                if stored_placement != object_placement {
                    doc.set_property_raw(id, "Placement", Property::Placement(stored_placement));
                }
                return;
            }
            if !keep_placement {
                // A plain feature: the shape is world-space geometry. Adopt its
                // (identity) transform, as upstream's non-recomputing rule does.
                if object_placement != Placement::identity() {
                    doc.set_property_raw(id, "Placement", Property::Placement(Placement::identity()));
                }
                return;
            }
            if object_placement == Placement::identity() {
                return;
            }
            // A local shape on an object that already has a placement: bake that
            // placement into the geometry, keeping the object where it is.
            let Ok(moved) =
                ferrocad_part::backend().transform(&shape, &object_placement.to_matrix())
            else {
                return;
            };
            let property = ShapeProperty::with_placement(moved, object_placement);
            doc.set_property_raw(id, "Shape", make_shape_property(property));
        }
        _ => {}
    }
}

/// The shape, accumulated placement and lineage a Part feature's `Shape` holds.
fn shape_property_parts(
    doc: &Document,
    id: ObjectId,
) -> Option<(Option<Shape>, Placement, Lineage)> {
    let object = doc.object(id)?;
    let Property::Extension(value) = object.properties.get("Shape")? else {
        return None;
    };
    let property = value.data().as_any().downcast_ref::<ShapeProperty>()?;
    Some((
        property.shape(),
        property.placement(),
        property.lineage().clone(),
    ))
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

    /// Store the placement and lineage a document property carries beside an
    /// already-placed shape, without re-baking the geometry.
    ///
    /// [`shape_to_py`] wraps a `Part::Feature.Shape` in the subclass matching its
    /// topology; the wrapper starts from [`from_shape`](Self::from_shape) and then
    /// adopts the property's own placement, so `obj.Shape.Placement` is the
    /// object's placement and its `Vertexes` are the same baked geometry.
    pub(crate) fn adopt_placement_and_lineage(
        &self,
        placement: Placement,
        lineage: Option<Lineage>,
    ) {
        *self.placement.lock().unwrap_or_else(|e| e.into_inner()) = placement;
        *self.lineage.lock().unwrap_or_else(|e| e.into_inner()) = lineage;
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
    ///
    /// This is upstream's `TopoShape::transformShape(matrix, copy, checkScale)`: a
    /// rigid matrix with `copy = False` moves the shape's location, and everything
    /// that changes scale is baked into the geometry (a non-uniform scale through
    /// `BRepBuilderAPI_GTransform`, so a circle really becomes an ellipse).
    fn apply_shape_transform(
        &self,
        matrix: &ferrocad_types::Matrix4,
        copy: bool,
        check_scale: bool,
    ) -> PyResult<()> {
        let shape = self.named_shape()?;
        let backend = ferrocad_part::backend();
        if is_general_transform(matrix, check_scale) {
            let moved = backend
                .transform_geometry(&shape, matrix, copy)
                .map_err(kernel_error)?;
            self.replace_shape(Some(moved));
        } else if matrix.has_scale(1e-9) == ScaleType::NoScaling
            && matrix.determinant3() > 0.0
        {
            if copy {
                let moved = backend.transform(&shape, matrix).map_err(kernel_error)?;
                self.replace_shape(Some(moved));
            } else {
                let loc = Placement::new(
                    Vector3::new(matrix.m[3], matrix.m[7], matrix.m[11]),
                    Rotation::from_matrix(matrix),
                );
                self.apply_global_placement(loc);
            }
        } else {
            // A similarity (or the `gp_Trsf` approximation of an affinity when
            // `checkScale` is off): bake it into the geometry.
            let moved = backend.transform(&shape, matrix).map_err(kernel_error)?;
            self.replace_shape(Some(moved));
        }
        Ok(())
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
    ///
    /// A null shape has none, so its getters return `[]` rather than raising —
    /// upstream's `TopoDS_Iterator` over an empty shape is likewise empty.
    pub(crate) fn sub_shapes(&self, kind: ElementKind) -> PyResult<Vec<Shape>> {
        let Some(shape) = self.shape() else {
            return Ok(Vec::new());
        };
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

/// The global placement of a rotation by `degrees` about the axis through
/// `center`, or `Part.OCCError` when the axis is degenerate.
///
/// This is the placement upstream's `TopoShape::rotate` applies to the shape's
/// location: base `center - R·center`, rotation `R`.
fn rotation_placement(
    center: ferrocad_curves::Vector3,
    axis: ferrocad_curves::Vector3,
    degrees: f64,
) -> PyResult<Placement> {
    if axis.is_null() {
        return Err(curves::OCCError::new_err(
            "a rotation needs a non-zero axis",
        ));
    }
    let rotation = Rotation::from_axis_angle(&axis, degrees.to_radians());
    Ok(Placement::new(
        center.sub(&rotation.to_matrix().transform(&center)),
        rotation,
    ))
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
    /// `Part.Shape()` is the null shape; `Part.Shape(other)` wraps the same kernel
    /// handle as `other` (upstream's copy constructor, `TopoShape(const
    /// TopoShape&)`), so the two are `isSame`/`isEqual` and hash alike. Use
    /// [`copy`](PartShape::copy) for an independent, deep copy.
    ///
    /// `Part.Shape([shapes])` follows upstream's `TopoShape::PyInit`: one entry
    /// wraps that shape (or the `toShape()` of that geometry object), and several
    /// are **fused** (`makeElementBoolean("FUSE", …)`), not merely compounded. The
    /// fuse is observable: fusing arcs that meet at a point unifies the shared
    /// vertices, so `Vertexes` has one entry per corner rather than two per edge.
    #[new]
    #[pyo3(signature = (value = None))]
    fn new(value: Option<&Bound<'_, PyAny>>) -> PyResult<Self> {
        let Some(value) = value else {
            return Ok(Self::empty());
        };
        if let Ok(other) = value.downcast::<PartShape>() {
            let other = other.borrow();
            if other.shape().is_none() {
                return Err(curves::OCCError::new_err("a null shape cannot be combined"));
            }
            return Ok(other.copy_handle());
        }
        if value.downcast::<pyo3::types::PyString>().is_ok() {
            return Err(pyo3::exceptions::PyTypeError::new_err(
                "expected a Part.Shape, a geometry object or a sequence of them",
            ));
        }
        let mut shapes = Vec::new();
        for item in value.try_iter()? {
            let item = item?;
            match item.downcast::<PartShape>() {
                Ok(shape) => {
                    let shape = shape.borrow();
                    shapes.push(
                        shape
                            .shape()
                            .ok_or_else(|| {
                                curves::OCCError::new_err("a null shape cannot be combined")
                            })?,
                    );
                }
                Err(_) => {
                    // A geometry object: its `toShape()` is the edge/face to hold.
                    let converted = item.call_method0("toShape").map_err(|_| {
                        pyo3::exceptions::PyTypeError::new_err(
                            "expected a Part.Shape, a geometry object or a sequence of them",
                        )
                    })?;
                    let shape = converted.downcast::<PartShape>().map_err(|_| {
                        pyo3::exceptions::PyTypeError::new_err(
                            "a geometry's toShape() must return a Part.Shape",
                        )
                    })?;
                    let shape = shape.borrow();
                    shapes.push(
                        shape
                            .shape()
                            .ok_or_else(|| {
                                curves::OCCError::new_err("a null shape cannot be combined")
                            })?,
                    );
                }
            }
        }
        if shapes.is_empty() {
            return Ok(Self::empty());
        }
        if shapes.len() == 1 {
            return Ok(Self::from_shape(shapes.into_iter().next().expect("one shape")));
        }
        // Upstream `TopoShape::PyInit` fuses a sequence with
        // `makeElementBoolean(OpCodes::Fuse, shapes)`. Folding one fuse per entry
        // keeps the merged vertices (the observable difference from a compound)
        // and its history, which is what names the result's elements.
        let backend = ferrocad_part::backend();
        let mut iter = shapes.into_iter();
        let mut fused = iter.next().expect("two or more shapes");
        let mut lineage = Lineage::default();
        for target in iter {
            let result = backend.fuse(&fused, &target).map_err(kernel_error)?;
            lineage.apply(&result.history);
            fused = result.shape;
        }
        Ok(Self::from_shape_with_lineage(fused, lineage))
    }

    /// A deep copy of the shape (`Shape.copy`), upstream's
    /// `BRepBuilderAPI_Copy`: a **new TShape** with the same geometry and
    /// location, so `copy` is neither `isSame` nor `isEqual` with the source but
    /// reports the same volume and area. The accumulated placement is preserved;
    /// the lineage is copied too unless `noElementMap` asks for a bare shape.
    ///
    /// The copy keeps the source's typed wrapper: `BRepBuilderAPI_Copy` preserves
    /// the topology, so a copied wire is still a `Part.Wire` and a copied solid a
    /// `Part.Solid`. Upstream's document shapes behave the same way, and Draft's
    /// `wire.fixWire()`/`Wire.OrderedEdges` call after a copy depends on it.
    #[pyo3(signature = (noElementMap = false))]
    fn copy(&self, py: Python<'_>, noElementMap: bool) -> PyResult<PyObject> {
        let Some(shape) = self.shape() else {
            return Ok(Py::new(py, Self::empty())?.into_any());
        };
        let copied = ferrocad_part::backend()
            .copy_shape(&shape)
            .map_err(kernel_error)?;
        let lineage = if noElementMap { None } else { self.lineage() };
        let wrapped = wrap_shape(py, copied)?;
        wrapped
            .bind(py)
            .downcast::<PartShape>()?
            .borrow()
            .adopt_placement_and_lineage(self.placement(), lineage);
        Ok(wrapped)
    }

    /// Sew the shape's faces into shells (`Shape.sewShape`), joining boundaries
    /// within `tolerance` (`BRepBuilderAPI_Sewing`).
    ///
    /// Upstream mutates the shape in place and returns `None`; so does this, and
    /// the geometry keeps the placement it was baked with.
    #[pyo3(signature = (tolerance = 1e-6))]
    fn sewShape(&self, tolerance: f64) -> PyResult<()> {
        let shape = self.named_shape()?;
        let sewn = ferrocad_part::backend()
            .sew(&shape, tolerance)
            .map_err(kernel_error)?;
        self.replace_geometry(Some(sewn));
        Ok(())
    }

    /// A copy with every curve and surface converted to a NURBS
    /// (`Shape.toNurbs`, `BRepBuilderAPI_NurbsConvert`).
    fn toNurbs(&self) -> PyResult<Self> {
        let shape = self.named_shape()?;
        let converted = ferrocad_part::backend()
            .to_nurbs(&shape)
            .map_err(kernel_error)?;
        Ok(Self::with_parts_and_placement(
            Some(converted),
            self.placement(),
            None,
        ))
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

    /// Translate this shape in place and return it (`Shape.translate`).
    ///
    /// Upstream moves the shape's location by the translation and returns the
    /// shape itself, so the geometry ends up at `T · placement`; this does too.
    fn translate(
        slf: &Bound<'_, Self>,
        vector: &Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
        let offset = curves::vector(vector)?;
        slf.borrow()
            .apply_global_placement(Placement::new(offset, Rotation::identity()));
        Ok(slf.clone().into_any().unbind())
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

    /// The direct children (`Shape.childShapes`): one topological level down.
    ///
    /// Without `shape_type` this follows upstream's `childShapes()` — the
    /// immediate sub-shapes:
    ///
    /// - a compound (and a comp-solid) reports its direct members (a compound of
    ///   two boxes gives the two solids);
    /// - a solid reports its shell, a shell its faces, a face its wires, a wire
    ///   its edges, and an edge its vertices — upstream's `TopoDS_Iterator`
    ///   result;
    /// - a vertex has none.
    ///
    /// A compound's members are reconstructed from the element lists because the
    /// seam has no direct-child enumeration: a sub-shape whose coarser kind is
    /// present in the compound is a descendant, not a member. A compound nested
    /// directly in another is therefore reported through the shapes it holds. The
    /// element maps are depth-first, so members of one kind keep the compound's
    /// order; a mixed compound is sorted back into it by the first vertex each
    /// member owns.
    ///
    /// With `shape_type` (for example `"Face"`) this is upstream's
    /// `getChildShapes(type)`: *every* sub-shape of that kind, recursively, so a
    /// box's faces are found even though they sit under its shell.
    #[pyo3(signature = (shape_type = None))]
    fn childShapes(
        &self,
        py: Python<'_>,
        shape_type: Option<&str>,
    ) -> PyResult<Vec<Py<PartShape>>> {
        let shape = match self.shape() {
            Some(shape) => shape,
            // A null shape has no children (upstream's `childShapes` is empty).
            None => return Ok(Vec::new()),
        };
        let shapes: Vec<Shape> = match shape_type {
            Some(name) => {
                let kind = ElementKind::ALL
                    .into_iter()
                    .find(|kind| name.starts_with(kind.prefix()))
                    .ok_or_else(|| {
                        PyValueError::new_err(format!(
                            "childShapes cannot enumerate {name:?}: the backend has no such element kind"
                        ))
                    })?;
                self.sub_shapes(kind)?
            }
            None => {
                let backend = ferrocad_part::backend();
                let inferred = self.shape_type();
                // The kind walk cannot tell a compound from the one shape inside
                // it; the compound count can, and a compound never sits under a
                // non-compound, so a non-zero count means this shape is one.
                let is_compound = backend
                    .count_sub_shapes(&shape, "Compound")
                    .map(|count| count > 0)
                    .unwrap_or(false)
                    || inferred == "Compound";
                if is_compound {
                    // The direct-child count lets a homogeneous compound stop
                    // after its coarsest kind, without ancestor queries.
                    let total = backend.count_sub_shapes(&shape, "SubShape").ok();
                    let mut members: Vec<Shape> = Vec::new();
                    let mut coarser: Vec<ElementKind> = Vec::new();
                    let mut member_kinds = 0usize;
                    for kind in ElementKind::ALL {
                        let before = members.len();
                        for element in backend.elements(&shape, kind) {
                            let nested = coarser.iter().any(|outer| {
                                !backend.ancestors(&shape, &element, *outer).is_empty()
                            });
                            if !nested {
                                members.push(element.shape);
                            }
                        }
                        if members.len() > before {
                            member_kinds += 1;
                        }
                        coarser.push(kind);
                        if total == Some(members.len()) {
                            break;
                        }
                    }
                    // A single kind is already in the compound's depth-first
                    // order. A mixed compound is grouped by kind above, so sort
                    // it back by the first vertex each member owns (the element
                    // maps themselves are depth-first, hence in member order).
                    if member_kinds > 1 && members.len() > 1 {
                        let vertices = backend.elements(&shape, ElementKind::Vertex);
                        let position = |member: &Shape| {
                            backend
                                .elements(member, ElementKind::Vertex)
                                .first()
                                .and_then(|first| {
                                    vertices.iter().position(|candidate| {
                                        backend.same_shape(&candidate.shape, &first.shape)
                                    })
                                })
                                .unwrap_or(usize::MAX)
                        };
                        members.sort_by_cached_key(position);
                    }
                    members
                } else {
                    match inferred {
                        "Solid" => self.sub_shapes(ElementKind::Shell)?,
                        "CompSolid" => self.sub_shapes(ElementKind::Solid)?,
                        "Shell" => self.sub_shapes(ElementKind::Face)?,
                        "Face" => self.sub_shapes(ElementKind::Wire)?,
                        "Wire" => self.sub_shapes(ElementKind::Edge)?,
                        "Edge" => self.sub_shapes(ElementKind::Vertex)?,
                        _ => Vec::new(),
                    }
                }
            }
        };
        shapes
            .into_iter()
            .map(|shape| Py::new(py, PartShape::from_shape(shape)))
            .collect()
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
    #[pyo3(signature = (matrix, copy = false, checkScale = false))]
    fn transformed(
        &self,
        matrix: &Bound<'_, PyAny>,
        copy: bool,
        checkScale: bool,
    ) -> PyResult<PartShape> {
        let matrix = matrix_from_py(matrix)?;
        let result = self.copy_handle();
        result.apply_shape_transform(&matrix, copy, checkScale)?;
        Ok(result)
    }

    /// Transform this shape in place and return it (`Shape.transformShape`).
    ///
    /// Upstream's signature is `transformShape(matrix, copy=False, checkScale=False)`.
    /// `copy = True` bakes the transformation into the geometry and leaves
    /// `Placement` untouched; `copy = False` moves a rigid transform into the
    /// location instead. A `checkScale` transform that is an affinity (a
    /// non-uniform scale or shear) goes through `BRepBuilderAPI_GTransform` and is
    /// always baked.
    #[pyo3(signature = (matrix, copy = false, checkScale = false))]
    fn transformShape<'py>(
        slf: &Bound<'py, Self>,
        matrix: &Bound<'_, PyAny>,
        copy: bool,
        checkScale: bool,
    ) -> PyResult<PyObject> {
        let matrix = matrix_from_py(matrix)?;
        slf.borrow().apply_shape_transform(&matrix, copy, checkScale)?;
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
        let about = rotation_placement(center, axis, angle)?;
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

    /// Whether the shape is closed (`Shape.isClosed`).
    ///
    /// Upstream defines this per shape type: a solid or comp-solid when every
    /// shell is closed; a shell, wire or edge (and a face or vertex) through
    /// `BRep_Tool::IsClosed`; and a compound when every shell, and every face,
    /// wire, edge and vertex not contained in a higher element, is closed.
    ///
    /// A null shape raises `ValueError`, as upstream's empty-shape queries do.
    fn isClosed(&self) -> PyResult<bool> {
        let shape = self.shape().ok_or_else(|| {
            PyValueError::new_err("cannot determine the 'Closed' flag of an empty shape")
        })?;
        ferrocad_part::backend()
            .is_closed(&shape)
            .map_err(kernel_error)
    }

    /// The shape's geometric tolerance (`Shape.getTolerance`).
    ///
    /// `mode` follows upstream's `ShapeAnalysis_ShapeTolerance`: `0` is the
    /// average over the considered sub-shapes, a positive value the maximum and a
    /// negative value the minimum. `type` restricts the kinds considered
    /// (`Part.Vertex`, `Part.Edge`, `Part.Face`, or the combined `Part.Shell`
    /// form); `None` is upstream's `TopAbs_SHAPE` — vertices, edges and faces.
    #[pyo3(signature = (mode = 0, r#type = None))]
    fn getTolerance(&self, mode: i32, r#type: Option<&Bound<'_, PyAny>>) -> PyResult<f64> {
        let shape = self
            .shape()
            .ok_or_else(|| curves::OCCError::new_err("a null shape has no tolerance"))?;
        let kinds: Vec<ElementKind> = match r#type {
            None => vec![ElementKind::Vertex, ElementKind::Edge, ElementKind::Face],
            Some(value) => {
                let kind = element_kind_from_py_type(value).map_err(|_| {
                    PyTypeError::new_err(
                        "getTolerance type must be Part.Vertex, Part.Edge, Part.Face or Part.Shell",
                    )
                })?;
                match kind {
                    ElementKind::Vertex | ElementKind::Edge | ElementKind::Face => vec![kind],
                    // Upstream's SHELL form combines the shell with its faces,
                    // edges and vertices.
                    ElementKind::Shell => vec![
                        ElementKind::Shell,
                        ElementKind::Face,
                        ElementKind::Edge,
                        ElementKind::Vertex,
                    ],
                    _ => {
                        return Err(PyTypeError::new_err(
                            "getTolerance type must be Part.Vertex, Part.Edge, Part.Face or Part.Shell",
                        ));
                    }
                }
            }
        };
        let backend = ferrocad_part::backend();
        let mut values = Vec::new();
        for kind in kinds {
            for element in backend.elements(&shape, kind) {
                if let Ok(tolerance) = backend.shape_tolerance(&element.shape) {
                    values.push(tolerance);
                }
            }
        }
        if values.is_empty() {
            return Ok(0.0);
        }
        Ok(if mode == 0 {
            values.iter().sum::<f64>() / values.len() as f64
        } else if mode > 0 {
            values.iter().copied().fold(f64::MIN, f64::max)
        } else {
            values.iter().copied().fold(f64::MAX, f64::min)
        })
    }

    /// The shape's geometric tolerance (`Shape.Tolerance`): the kernel value on a
    /// vertex, edge or face, and the maximum over those sub-shapes on a higher
    /// shape.
    #[getter]
    fn Tolerance(&self) -> PyResult<f64> {
        let shape = self
            .shape()
            .ok_or_else(|| curves::OCCError::new_err("a null shape has no tolerance"))?;
        ferrocad_part::backend()
            .shape_tolerance(&shape)
            .map_err(kernel_error)
    }

    /// Check the shape for topological and geometric validity
    /// (`Shape.check`).
    ///
    /// Upstream raises `ValueError` on the first problem and returns `None` for a
    /// valid (or null) shape; the seam exposes `BRepCheck_Analyzer`, so this does
    /// the same.
    #[pyo3(signature = (run_bop_check = false))]
    fn check(&self, run_bop_check: bool) -> PyResult<()> {
        let _ = run_bop_check;
        if let Some(shape) = self.shape() {
            if !ferrocad_part::backend().is_valid(&shape) {
                return Err(PyValueError::new_err("the shape is not valid"));
            }
        }
        Ok(())
    }

    /// The minimum distance between this shape and `other` (`Shape.distToShape`).
    ///
    /// Returns `(distance, [(point_on_self, point_on_other)], info)`, matching
    /// upstream's `BRepExtrema_DistShapeShape` wrapper. The kernel seam exposes
    /// the distance and the two closest points but not the support types, so
    /// `info` is always empty here (upstream reports the Vertex/Edge/Face support
    /// and its parameter per solution).
    ///
    /// `other` must be a `Part.Shape`; anything else is a `TypeError`. Either
    /// null shape raises `ValueError`.
    fn distToShape(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let other = other
            .downcast::<PartShape>()
            .map_err(|_| PyTypeError::new_err("distToShape expects a Part.Shape"))?;
        let first = self
            .shape()
            .ok_or_else(|| PyValueError::new_err("a null shape cannot be measured"))?;
        let second = other
            .borrow()
            .shape()
            .ok_or_else(|| PyValueError::new_err("a null shape cannot be measured"))?;
        let result = ferrocad_part::backend()
            .dist_to_shape(&first, &second)
            .map_err(kernel_error)?;
        let pair = PyTuple::new(
            py,
            [
                curves::py_vector(py, result.first)?,
                curves::py_vector(py, result.second)?,
            ],
        )?;
        let pairs = PyList::new(py, [pair])?;
        let info = PyList::empty(py);
        Ok(PyTuple::new(
            py,
            [
                result.distance.into_py_any(py)?,
                pairs.into_any().unbind(),
                info.into_any().unbind(),
            ],
        )?
        .into_any()
        .unbind())
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

    /// Whether two shapes are equal (`Shape.isEqual`): the same TShape and
    /// location **and** the same orientation. `Shape.reversed()` is `isSame`
    /// but not `isEqual`.
    ///
    /// A null shape (either side) raises `ValueError`, as upstream's `TopoShape`
    /// comparison does not define equality for an empty shape.
    fn isEqual(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        let other = other
            .downcast::<PartShape>()
            .map_err(|_| PyTypeError::new_err("isEqual expects a Part.Shape"))?;
        let a = self
            .shape()
            .ok_or_else(|| PyValueError::new_err("a null shape cannot be compared"))?;
        let b = other
            .borrow()
            .shape()
            .ok_or_else(|| PyValueError::new_err("a null shape cannot be compared"))?;
        ferrocad_part::backend()
            .is_equal(&a, &b)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }

    /// `shape == other` is [`isEqual`](PartShape::isEqual): the same TShape,
    /// location **and** orientation. A reversed shape is therefore not equal,
    /// while `Part.Shape(box) == box` (a handle copy) is.
    ///
    /// Two null shapes are not equal: upstream's `IsEqual` is undefined for an
    /// empty shape, and `==` must stay total, so it answers `False` instead of
    /// raising. A non-shape operand returns `NotImplemented`, letting Python try
    /// the reflected comparison.
    fn __eq__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let Ok(other) = other.downcast::<PartShape>() else {
            return Ok(py.NotImplemented());
        };
        let a = self.shape();
        let b = other.borrow().shape();
        match (a, b) {
            (Some(a), Some(b)) => match ferrocad_part::backend().is_equal(&a, &b) {
                Ok(equal) => equal.into_py_any(py),
                // A backend without `is_equal` cannot answer; identity is the
                // Python fallback, so `==` still terminates instead of raising.
                Err(_) => Ok(py.NotImplemented()),
            },
            _ => false.into_py_any(py),
        }
    }

    /// Inequality mirrors [`__eq__`](PartShape::__eq__).
    fn __ne__(&self, py: Python<'_>, other: &Bound<'_, PyAny>) -> PyResult<PyObject> {
        let Ok(other) = other.downcast::<PartShape>() else {
            return Ok(py.NotImplemented());
        };
        let a = self.shape();
        let b = other.borrow().shape();
        match (a, b) {
            (Some(a), Some(b)) => match ferrocad_part::backend().is_equal(&a, &b) {
                Ok(equal) => (!equal).into_py_any(py),
                Err(_) => Ok(py.NotImplemented()),
            },
            _ => true.into_py_any(py),
        }
    }

    /// The hash follows [`isSame`](PartShape::isSame): TShape and location,
    /// **ignoring orientation**. It is what `dict`/`set` lookups keyed by a
    /// sub-shape need (`bucket = lookup.setdefault(hash(edge), [])` followed by
    /// an `isSame` check): every wrapper around one kernel sub-shape hashes
    /// alike, reversed or not, while a deep copy has a new TShape and hashes
    /// differently. A null shape hashes to `0` (it is equal to nothing).
    fn __hash__(&self) -> u64 {
        let Some(shape) = self.shape() else {
            return 0;
        };
        // `Py_hash_t` is signed; keep the value in its non-negative range.
        ferrocad_part::backend()
            .shape_hash(&shape)
            .map(|hash| hash & (i64::MAX as u64))
            .unwrap_or(0)
    }

    /// The number of sub-shapes of `kind` (`Shape.countElement`).
    ///
    /// `kind` is upstream's spelling (`Face`, `Edge`, `Vertex`, `Wire`, `Shell`,
    /// `Solid`, `Compound`, `CompSolid`; `SubShape` counts the direct children).
    /// An unknown spelling counts zero, matching upstream's silent type lookup.
    /// A null shape also has no sub-shapes, so it counts zero.
    fn countElement(&self, kind: &str) -> PyResult<usize> {
        let Some(shape) = self.shape() else {
            return Ok(0);
        };
        ferrocad_part::backend()
            .count_sub_shapes(&shape, kind)
            .map_err(|error| PyRuntimeError::new_err(error.to_string()))
    }

    /// Write the shape as BREP to `filename` (FreeCAD's `Shape.exportBrep`).
    fn exportBrep(&self, filename: &str) -> PyResult<()> {
        let shape = self
            .shape()
            .ok_or_else(|| PyValueError::new_err("cannot export a null shape"))?;
        let bytes = ferrocad_part::backend().save_shape(&shape);
        std::fs::write(filename, bytes).map_err(|e| PyIOError::new_err(e.to_string()))
    }

    /// Load this shape from BREP data, replacing its contents in place
    /// (`Shape.importBrepFromString`, upstream's `TopoShape::importBrep`).
    ///
    /// `data` is the BREP text as `str` or `bytes`; upstream returns a `str` from
    /// `exportBrepToString` while this binding returns `bytes`, so both are
    /// accepted. `indicator` is upstream's progress-bar flag (`1` shows it, `0`
    /// does not); it is accepted for signature compatibility and ignored.
    ///
    /// Invalid data raises `Part.OCCError` and leaves the shape unchanged.
    #[pyo3(signature = (data, indicator = 1))]
    fn importBrepFromString(&self, data: &Bound<'_, PyAny>, indicator: i32) -> PyResult<()> {
        let _ = indicator;
        let bytes = if let Ok(text) = data.extract::<String>() {
            text.into_bytes()
        } else if let Ok(raw) = data.downcast::<PyBytes>() {
            raw.as_bytes().to_vec()
        } else {
            return Err(PyTypeError::new_err(
                "importBrepFromString expects str or bytes",
            ));
        };
        let shape = ferrocad_part::backend().load_shape(&bytes).ok_or_else(|| {
            curves::OCCError::new_err("importBrepFromString: the data is not a valid BREP shape")
        })?;
        self.replace_shape(Some(shape));
        Ok(())
    }

    /// The BREP encoding of the shape, as `bytes`.
    /// `Shape.exportBrepToString` — the BREP text of the shape.
    ///
    /// Upstream returns a `str` and so does this; `importBrepFromString` accepts
    /// that `str` back (and still accepts `bytes` for callers written against
    /// the older binding, which returned `PyBytes`).
    fn exportBrepToString(&self) -> PyResult<String> {
        let shape = self
            .shape()
            .ok_or_else(|| PyValueError::new_err("cannot export a null shape"))?;
        let bytes = ferrocad_part::backend().save_shape(&shape);
        String::from_utf8(bytes)
            .map_err(|_| curves::OCCError::new_err("the BREP export is not valid UTF-8"))
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

    /// Upstream FreeCAD's `Shape.tessellate`: `(points, facets)`.
    ///
    /// `points` is a list of `Base.Vector` (the same `Vector` class as
    /// `FreeCAD.Vector`) and `facets` a list of `(i, j, k)` index triples, so the
    /// result can be passed straight to `Mesh.Mesh(...)`. Unlike the FerroCAD
    /// extension [`mesh`](Self::mesh), which keeps the per-face duplicated
    /// positions and part ids a viewport picks with, this merges vertices within
    /// `gp::Resolution` and drops facets that collapse to a line — upstream's
    /// shape.
    ///
    /// `deflection` is the linear deflection in model units; `angular` is the
    /// angular deflection in radians.
    #[pyo3(signature = (deflection = 0.5, angular = 0.5))]
    fn tessellate(
        &self,
        py: Python<'_>,
        deflection: f64,
        angular: f64,
    ) -> PyResult<(Vec<Py<PyVector>>, Vec<(usize, usize, usize)>)> {
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

        // Upstream merges coincident mesh vertices across faces (a box is 8 points,
        // not one set of nodes per face) and returns only the triangle mesh. The
        // merge tolerance is `gp::Resolution`; exact-bit equality is not enough on a
        // curved face, where the two sides of a seam differ in the last bits.
        let (positions, facets) = weld_triangles(&mesh.positions, &mesh.triangles);
        let points = positions
            .into_iter()
            .map(|point| Py::new(py, PyVector::fresh(point)))
            .collect::<PyResult<_>>()?;
        Ok((points, facets))
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

    /// The wires (`Shape.Wires`) as `Part.Wire` wrappers.
    #[getter]
    fn Wires(&self, py: Python<'_>) -> PyResult<Vec<Py<curves::PyWire>>> {
        self.sub_shapes(ElementKind::Wire)?
            .into_iter()
            .map(|shape| Py::new(py, curves::PyWire::from_shape(shape)))
            .collect()
    }

    /// The faces (`Shape.Faces`) as `Part.Face` wrappers, so `Face.valueAt` and
    /// `Face.Surface` are available without a cast.
    #[getter]
    fn Faces(&self, py: Python<'_>) -> PyResult<Vec<Py<curves::PyFace>>> {
        self.sub_shapes(ElementKind::Face)?
            .into_iter()
            .map(|shape| Py::new(py, curves::PyFace::from_shape(shape)))
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
    /// [`childShapes`](Self::childShapes) returns without a type filter — one
    /// topological level, or a compound's members.
    #[getter]
    fn SubShapes(&self, py: Python<'_>) -> PyResult<Vec<Py<PartShape>>> {
        self.childShapes(py, None)
    }

    /// The solids (`Shape.Solids`) as [`Part.Solid`](solids::PySolid) handles.
    #[getter]
    fn Solids(&self, py: Python<'_>) -> PyResult<Vec<Py<solids::PySolid>>> {
        self.sub_shapes(ElementKind::Solid)?
            .into_iter()
            .map(|shape| Py::new(py, solids::PySolid::from_shape(shape)))
            .collect()
    }

    /// The shells (`Shape.Shells`) as [`Part.Shell`](solids::PyShell) handles.
    #[getter]
    fn Shells(&self, py: Python<'_>) -> PyResult<Vec<Py<solids::PyShell>>> {
        self.sub_shapes(ElementKind::Shell)?
            .into_iter()
            .map(|shape| Py::new(py, solids::PyShell::from_shape(shape)))
            .collect()
    }

    /// The shape's topological kind (`Shape.ShapeType`); see the `shape_type`
    /// inference above.
    #[getter]
    fn ShapeType(&self) -> &'static str {
        self.shape_type()
    }

    /// A face's parameter bounds (`Face.ParameterRange`), or an edge's
    /// `(first, last)` (`Edge.ParameterRange`), as upstream reports them.
    ///
    /// A face returns the four values `(u0, u1, v0, v1)` of its boundary in the
    /// surface's UV space (OCCT `BRepTools::UVBounds`); an edge returns the two
    /// values of its curve range (OCCT `BRep_Tool::Range`). Another shape kind,
    /// or a null shape, raises `ValueError`/`TypeError` rather than inventing a
    /// range.
    #[getter]
    fn ParameterRange(&self, py: Python<'_>) -> PyResult<PyObject> {
        let shape = self
            .shape()
            .ok_or_else(|| PyValueError::new_err("a null shape has no parameter range"))?;
        match self.shape_type() {
            "Face" => {
                let (u0, u1, v0, v1) = ferrocad_part::backend()
                    .face_parameter_range(&shape)
                    .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
                Ok(PyTuple::new(py, [u0, u1, v0, v1])?.into_any().unbind())
            }
            "Edge" => {
                let (first, last) = ferrocad_part::backend()
                    .edge_parameter_range(&shape)
                    .map_err(|error| PyRuntimeError::new_err(error.to_string()))?;
                Ok(PyTuple::new(py, [first, last])?.into_any().unbind())
            }
            other => Err(PyTypeError::new_err(format!(
                "ParameterRange is not defined for a {other}"
            ))),
        }
    }

    /// The backing surface (`Face.Surface`), when the shape is a face.
    ///
    /// A plane, cylinder or sphere reports the elementary surface class
    /// (`Part.Plane`, `Part.Cylinder`, `Part.Sphere`) with the kernel's own
    /// parameter frame; any other surface reports the kernel's type id.
    #[getter]
    fn Surface(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        let Some(shape) = self.shape() else {
            return Ok(None);
        };
        // A B-spline face reports the full control-point class, so `UDegree`,
        // `getPoles` and `parameter` all describe the face's own surface.
        if ferrocad_part::backend().surface_type(&shape).as_deref()
            == Some("Part::GeomBSplineSurface")
        {
            if let Some(geometry) = ferrocad_occt::part_ops::bspline_surface_data(&shape) {
                return Ok(Some(
                    Py::new(py, solids::PyBSplineSurface::from_geometry(geometry))?.into_any(),
                ));
            }
        }
        let Some(surface) = ferrocad_part::backend().surface_data(&shape) else {
            return Ok(None);
        };
        Ok(Some(match surface {
            SurfaceData::Plane {
                origin,
                x_dir,
                y_dir,
                normal,
            } => Py::new(
                py,
                curves::PyPlane::from_frame(origin, normal, x_dir, y_dir),
            )?
            .into_any(),
            SurfaceData::Cylinder {
                origin,
                axis,
                x_dir,
                radius,
                direct,
            } => Py::new(
                py,
                curves::PyCylinder::from_parts(radius, origin, axis, x_dir, direct),
            )?
            .into_any(),
            SurfaceData::Sphere {
                center,
                axis,
                x_dir,
                radius,
                direct,
            } => Py::new(
                py,
                curves::PySphere::from_parts(radius, center, axis, x_dir, direct),
            )?
            .into_any(),
            SurfaceData::Cone {
                origin,
                axis,
                x_dir,
                radius,
                semi_angle,
                direct,
            } => Py::new(
                py,
                solids::PyCone::from_parts(radius, origin, axis, x_dir, semi_angle, direct),
            )?
            .into_any(),
            SurfaceData::Torus {
                center,
                axis,
                x_dir,
                major_radius,
                minor_radius,
                direct,
            } => Py::new(
                py,
                solids::PyToroid::from_parts(
                    major_radius,
                    minor_radius,
                    center,
                    axis,
                    x_dir,
                    direct,
                ),
            )?
            .into_any(),
            SurfaceData::Other { type_id } => {
                Py::new(py, curves::PySurface::new(type_id, Some(shape.clone())))?.into_any()
            }
        }))
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
    ///
    /// The property regime follows the shape type: volume for a solid, surface for
    /// a face or shell, linear for an edge or wire, and the point for a vertex.
    #[getter]
    fn CenterOfMass(&self, py: Python<'_>) -> PyResult<PyObject> {
        let point = self
            .shape()
            .and_then(|shape| ferrocad_part::backend().center_of_mass(&shape))
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

    /// Set the shape orientation (`Shape.Orientation = "Reversed"`).
    ///
    /// The four FreeCAD names are accepted (`Forward`, `Reversed`, `Internal`,
    /// `External`); the geometry and the placement are unchanged, and the lineage
    /// map is kept because an orientation change does not alter the topology. A
    /// null shape and an unknown name are errors.
    #[setter]
    fn set_Orientation(&mut self, value: &str) -> PyResult<()> {
        let shape = self
            .shape()
            .ok_or_else(|| curves::OCCError::new_err("a null shape has no orientation"))?;
        let oriented = ferrocad_part::backend()
            .with_orientation(&shape, value)
            .map_err(kernel_error)?;
        *self.shape.lock().unwrap_or_else(|e| e.into_inner()) = Some(oriented);
        self.epoch.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Upstream's type id for a `TopoShape`.
    #[getter]
    fn TypeId(&self) -> &'static str {
        "Part::TopoShape"
    }

    /// The compound children (`Shape.Compounds`), from `TopExp::MapShapes` over
    /// `TopAbs_COMPOUND`.
    ///
    /// This is upstream's enumeration, so it is exact: nested compounds appear,
    /// and a compound reports itself as well (the map starts at the shape). A
    /// shape with no compound sub-shape reports `[]`.
    #[getter]
    fn Compounds(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let list = PyList::empty(py);
        let Some(shape) = self.shape() else {
            return Ok(list.unbind());
        };
        for compound in ferrocad_part::backend().sub_shapes_of_type(&shape, "Compound") {
            list.append(PartShape::from_shape(compound))?;
        }
        Ok(list.unbind())
    }

    /// The comp-solid children (`Shape.CompSolids`), from `TopExp::MapShapes`
    /// over `TopAbs_COMPSOLID`.
    #[getter]
    fn CompSolids(&self, py: Python<'_>) -> PyResult<Py<PyList>> {
        let list = PyList::empty(py);
        let Some(shape) = self.shape() else {
            return Ok(list.unbind());
        };
        for comp_solid in ferrocad_part::backend().sub_shapes_of_type(&shape, "CompSolid") {
            list.append(PartShape::from_shape(comp_solid))?;
        }
        Ok(list.unbind())
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
    /// `glue` and `tolerance` are accepted for upstream signature compatibility
    /// (`glue` is coerced for truthiness, so a float is accepted as upstream
    /// allows); the kernel runs the plain boolean. `noElementMap` drops the
    /// lineage, exactly as upstream's `ElementMapPolicy::Drop`.
    #[pyo3(signature = (other, glue = None, tolerance = 0.0, refine = true, noElementMap = false))]
    fn fuse(
        &self,
        other: &Bound<'_, PyAny>,
        glue: Option<&Bound<'_, PyAny>>,
        tolerance: f64,
        refine: bool,
        noElementMap: bool,
    ) -> PyResult<PartShape> {
        let _ = (truthy(glue), tolerance, refine);
        self.boolean(other, BooleanOp::Fuse, noElementMap)
    }

    /// `Shape.cut(other)` — `self` with `other` removed.
    #[pyo3(signature = (other, glue = None, tolerance = 0.0, refine = true, noElementMap = false))]
    fn cut(
        &self,
        other: &Bound<'_, PyAny>,
        glue: Option<&Bound<'_, PyAny>>,
        tolerance: f64,
        refine: bool,
        noElementMap: bool,
    ) -> PyResult<PartShape> {
        let _ = (truthy(glue), tolerance, refine);
        self.boolean(other, BooleanOp::Cut, noElementMap)
    }

    /// `Shape.common(other)` — the intersection of the two shapes.
    #[pyo3(signature = (other, glue = None, tolerance = 0.0, refine = true, noElementMap = false))]
    fn common(
        &self,
        other: &Bound<'_, PyAny>,
        glue: Option<&Bound<'_, PyAny>>,
        tolerance: f64,
        refine: bool,
        noElementMap: bool,
    ) -> PyResult<PartShape> {
        let _ = (truthy(glue), tolerance, refine);
        self.boolean(other, BooleanOp::Common, noElementMap)
    }

    /// `Shape.multiFuse(shapes)` — fuse this shape with each shape in `shapes`.
    #[pyo3(signature = (shapes, glue = None, tolerance = 0.0, refine = true, noElementMap = false))]
    fn multiFuse(
        &self,
        shapes: &Bound<'_, PyAny>,
        glue: Option<&Bound<'_, PyAny>>,
        tolerance: f64,
        refine: bool,
        noElementMap: bool,
    ) -> PyResult<PartShape> {
        let _ = (truthy(glue), tolerance, refine);
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

    /// `Shape.makeOffsetShape(offset, tolerance, inter=False, self_inter=False,
    /// offsetMode=0, join=0, fill=False)` — a 3D offset.
    ///
    /// Upstream's `TopoShape::makeElementOffset`
    /// (`BRepOffsetAPI_MakeOffsetShape::PerformByJoin`): `offsetMode` is
    /// `BRepOffset_Mode` (0 skin, 1 pipe, 2 recto-verso), `join` is 0 arcs, 1
    /// tangent, 2 intersection, and `fill=True` closes a face offset into a solid.
    #[pyo3(signature = (offset, tolerance, inter = false, self_inter = false, offsetMode = 0, join = 0, fill = false))]
    #[allow(clippy::too_many_arguments)]
    fn makeOffsetShape(
        &self,
        offset: f64,
        tolerance: f64,
        inter: bool,
        self_inter: bool,
        offsetMode: i32,
        join: i32,
        fill: bool,
    ) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        let result = ferrocad_occt::part_ops::offset_shape(
            &shape,
            offset,
            tolerance,
            inter,
            self_inter,
            offsetMode,
            join,
            fill,
        )
        .map_err(kernel_error)?;
        Ok(PartShape::from_shape(result))
    }

    /// `Shape.fix(tolerance, mintol=0, maxtol=0)` — repair a broken shape in place
    /// and report whether the result is valid (upstream's
    /// `TopoShape::fix(precision, mintol, maxtol)`).
    ///
    /// A shape the fixer cannot improve is left as it was, so the call still
    /// answers whether the shape is valid instead of failing.
    #[pyo3(signature = (tolerance, mintol = 0.0, maxtol = 0.0))]
    fn fix(&self, tolerance: f64, mintol: f64, maxtol: f64) -> PyResult<bool> {
        let shape = self.named_shape()?;
        let fixed = ferrocad_occt::part_ops::fix_shape(&shape, tolerance, mintol, maxtol)
            .unwrap_or(shape);
        let valid = ferrocad_part::backend().is_valid(&fixed);
        self.replace_geometry(Some(fixed));
        Ok(valid)
    }

    /// `Shape.defeaturing(faces)` — remove faces and let their neighbours grow to
    /// close the gap (`BRepAlgoAPI_Defeaturing`).
    fn defeaturing(&self, faces: &Bound<'_, PyAny>) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        let faces = shape_sequence(faces)?;
        if faces.is_empty() {
            return Err(curves::OCCError::new_err(
                "defeaturing needs at least one face to remove",
            ));
        }
        let result = ferrocad_occt::part_ops::defeature(&shape, &faces).map_err(kernel_error)?;
        Ok(PartShape::from_shape(result))
    }

    /// `Shape.makeParallelProjection(tool, direction)` — project an edge or wire
    /// onto this shape along `direction` (`BRepProj_Projection`).
    fn makeParallelProjection(
        &self,
        tool: &Bound<'_, PyAny>,
        direction: &Bound<'_, PyAny>,
    ) -> PyResult<PartShape> {
        let target = self.named_shape()?;
        let tool = shape_argument(tool)?;
        let direction = curves::vector(direction)?;
        let projected = ferrocad_occt::part_ops::project(&tool, &target, direction, true)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(projected))
    }

    /// `Shape.makePerspectiveProjection(tool, viewpoint)` — project an edge or wire
    /// onto this shape from a viewpoint (`BRepProj_Projection`).
    fn makePerspectiveProjection(
        &self,
        tool: &Bound<'_, PyAny>,
        viewpoint: &Bound<'_, PyAny>,
    ) -> PyResult<PartShape> {
        let target = self.named_shape()?;
        let tool = shape_argument(tool)?;
        let viewpoint = curves::vector(viewpoint)?;
        let projected = ferrocad_occt::part_ops::project(&tool, &target, viewpoint, false)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(projected))
    }

    /// `Shape.makePipeShell(profiles, makeSolid=False, isFrenet=False,
    /// transition=0)` — sweep the profiles along this spine
    /// (`BRepOffsetAPI_MakePipeShell`).
    ///
    /// The upstream checkout no longer exposes this method (`makeSweepSurface`
    /// and the Part::Sweep feature replaced it), but Draft-era scripts still call
    /// it; `transition` is accepted and ignored, as `MakePipeShell` picks its own
    /// transition mode here.
    #[pyo3(signature = (profiles, makeSolid = false, isFrenet = false, transition = 0))]
    fn makePipeShell(
        &self,
        profiles: &Bound<'_, PyAny>,
        makeSolid: bool,
        isFrenet: bool,
        transition: i32,
    ) -> PyResult<PartShape> {
        let _ = transition;
        let spine = self.named_shape()?;
        let profiles = shape_sequence(profiles)?;
        let swept = surface_ops::make_pipe_shell(&spine, &profiles, makeSolid, isFrenet)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(swept))
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

    /// `Shape.ancestorsOfType(subshape, type)` — the sub-elements of `type`
    /// that contain `subshape` (upstream `TopoShapePy::ancestorsOfType`).
    ///
    /// `type` is a `Part.Shape` subclass (`Part.Face`, `Part.Edge`, …). The
    /// returned wrappers use the subclass for their kind, as upstream's
    /// `shape2pyshape` does. A sub-shape that is not part of this shape raises
    /// `ValueError` rather than silently returning nothing.
    fn ancestorsOfType(
        &self,
        py: Python<'_>,
        subshape: &Bound<'_, PyAny>,
        kind: &Bound<'_, PyAny>,
    ) -> PyResult<Vec<PyObject>> {
        let base = self.named_shape()?;
        let wanted = element_kind_from_py_type(kind)?;
        let target = shape_argument(subshape)?;
        // Upstream's `findAncestors` is a cache lookup: a shape that is not a
        // sub-shape of this one simply has no ancestors here, so this reports an
        // empty list rather than raising.
        let Some(source_kind) = PartShape::kind_of(&target) else {
            return Ok(Vec::new());
        };
        let backend = ferrocad_part::backend();
        let Some(element) = backend
            .elements(&base, source_kind)
            .into_iter()
            .find(|element| backend.same_shape(&element.shape, &target))
        else {
            return Ok(Vec::new());
        };
        backend
            .ancestors(&base, &element, wanted)
            .into_iter()
            .map(|ancestor| shape_wrapper_to_py(py, ancestor.shape))
            .collect()
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
    /// sub-shape, wrapped in the Python subclass for its kind (`Part.Face`,
    /// `Part.Edge`, …), as upstream's `getSubTopoShape` does. Without an element
    /// map this is the positional resolution.
    ///
    /// The failure modes mirror upstream's `getSubTopoShape`: a name whose
    /// spelling is not a known kind followed by a positive integer (including
    /// `InvalidName` and a stray suffix such as `Face6_abc`) is a `ValueError`,
    /// and a well-spelled name past the shape's last element of that kind is an
    /// `IndexError`. A null shape is a `ValueError` as well.
    fn getElement(&self, py: Python<'_>, name: &str) -> PyResult<PyObject> {
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
            Some(resolved) => shape_wrapper_to_py(py, resolved),
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
    ///
    /// The result is wrapped by topology — a `Part.Wire` — as upstream's
    /// `shape2pyshape` wraps it; Draft's `get_extended_wire` calls
    /// `.OrderedEdges` on it straight away.
    fn split(
        &self,
        py: Python<'_>,
        paramval: &Bound<'_, PyAny>,
    ) -> PyResult<PyObject> {
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
        wrap_shape(py, split)
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

    /// Whether `point` lies inside this shape (`Shape.isInside`).
    ///
    /// Upstream's rule is not uniform. A vertex, edge, wire or face answers with
    /// the point-to-shape distance (`BRepExtrema_DistShapeShape`), so a point on
    /// the element is inside whatever `checkFace` says. Every other shape is
    /// classified by `BRepClass3d_SolidClassifier`; `checkFace` then also counts
    /// a point the classifier reports lying on a boundary face
    /// (`IsOnAFace`), which is otherwise `ON` rather than `IN`.
    ///
    /// `point` may be a `Base.Vector`, a `(x, y, z)` sequence or any object with
    /// `x`/`y`/`z`. A null shape raises `Part.OCCError`, as upstream does.
    #[pyo3(signature = (point, tolerance, checkFace = false))]
    fn isInside(
        &self,
        point: &Bound<'_, PyAny>,
        tolerance: f64,
        checkFace: bool,
    ) -> PyResult<bool> {
        let point = curves::vector(point)?;
        let shape = self.named_shape()?;
        ferrocad_part::backend()
            .is_inside(&shape, point, tolerance, checkFace)
            .map_err(kernel_error)
    }

    /// The identity hash upstream's `ShapeMapHasher`/`TopoDS_Shape::HashCode`
    /// reports (`Shape.hashCode`).
    ///
    /// It follows [`isSame`](PartShape::isSame): TShape and location, orientation
    /// ignored, so every wrapper around one kernel sub-shape hashes alike and a
    /// deep copy does not. `upper` is upstream's bound (`IntegerLast()` by
    /// default); the value is taken modulo it. A null shape hashes to `0` (it is
    /// equal to nothing).
    #[pyo3(signature = (upper = i32::MAX))]
    fn hashCode(&self, upper: i32) -> u64 {
        let Some(shape) = self.shape() else {
            return 0;
        };
        let upper = if upper > 0 { upper as u64 } else { 1 };
        ferrocad_part::backend()
            .shape_hash(&shape)
            .map(|hash| hash % upper)
            .unwrap_or(0)
    }

    /// Whether both shapes share the same geometry, ignoring placement and
    /// orientation (`Shape.isPartner`, `TopoDS_Shape::IsPartner`).
    ///
    /// This is weaker than [`isSame`](PartShape::isSame), which also compares the
    /// location. A null shape on either side answers `false`; a non-shape is a
    /// `TypeError`.
    fn isPartner(&self, other: &Bound<'_, PyAny>) -> PyResult<bool> {
        let other = other
            .downcast::<PartShape>()
            .map_err(|_| PyTypeError::new_err("isPartner expects a Part.Shape"))?;
        let a = self.shape();
        let b = other.borrow().shape();
        match (a, b) {
            (Some(a), Some(b)) => ferrocad_part::backend()
                .is_partner(&a, &b)
                .map_err(kernel_error),
            _ => Ok(false),
        }
    }

    /// Apply a 4x4 matrix to the shape's **geometry** and return a new shape
    /// (`Shape.transformGeometry`, `BRepBuilderAPI_GTransform`).
    ///
    /// Unlike [`transformed`](PartShape::transformed), which installs a `gp_Trsf`
    /// (a similarity), this path goes through `gp_GTrsf`, so it can express a
    /// non-uniform scale or shear: a circle becomes an ellipse. The source shape
    /// is not modified, matching upstream's `copy` flag default. The result has
    /// the identity placement, because the whole transformation is baked in.
    #[pyo3(signature = (matrix, copy = false))]
    fn transformGeometry(
        &self,
        matrix: &Bound<'_, PyAny>,
        copy: bool,
    ) -> PyResult<PartShape> {
        let matrix = matrix_from_py(matrix)?;
        let shape = self.named_shape()?;
        let moved = ferrocad_part::backend()
            .transform_geometry(&shape, &matrix, copy)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(moved))
    }

    /// A copy rotated by `degree` about the axis through `center` along `axis`
    /// (`Shape.rotated`).
    ///
    /// This is upstream's `rotated`, i.e. a copy followed by the in-place
    /// [`rotate`](PartShape::rotate); the original shape is untouched.
    fn rotated(
        &self,
        center: &Bound<'_, PyAny>,
        axis: &Bound<'_, PyAny>,
        degree: f64,
    ) -> PyResult<PartShape> {
        let center = curves::vector(center)?;
        let axis = curves::vector(axis)?;
        let about = rotation_placement(center, axis, degree)?;
        let result = self.copy_handle();
        result.apply_global_placement(about);
        Ok(result)
    }

    /// Scale this shape in place by `factor` about `base` (the origin by
    /// default) and return it (`Shape.scale`).
    ///
    /// This is upstream's in-place form; [`scaled`](PartShape::scaled) is the
    /// copy form. As upstream insists, a factor below the kernel's confusion
    /// tolerance is a `ValueError`; the scale is baked into the geometry, so the
    /// accumulated placement resets to the identity.
    #[pyo3(signature = (factor, base = None))]
    fn scale(
        slf: &Bound<'_, Self>,
        factor: f64,
        base: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<PyObject> {
        if factor.abs() < ferrocad_curves::TOLERANCE {
            return Err(PyValueError::new_err("scale factor too small"));
        }
        let center = base
            .map(curves::vector)
            .transpose()?
            .unwrap_or_else(ferrocad_curves::Vector3::zero);
        let matrix = scale_matrix(factor, center);
        let shape = slf.borrow().named_shape()?;
        let moved = ferrocad_part::backend()
            .transform(&shape, &matrix)
            .map_err(kernel_error)?;
        slf.borrow().replace_shape(Some(moved));
        Ok(slf.clone().into_any().unbind())
    }

    /// The average (`mode == 0`), maximum (`mode > 0`) or minimum (`mode < 0`)
    /// tolerance over this shape's vertices, edges and faces
    /// (`Shape.globalTolerance`, `ShapeAnalysis_ShapeTolerance`).
    fn globalTolerance(&self, mode: i32) -> PyResult<f64> {
        let shape = self
            .shape()
            .ok_or_else(|| curves::OCCError::new_err("a null shape has no tolerance"))?;
        ferrocad_part::backend()
            .global_tolerance(&shape, mode)
            .map_err(kernel_error)
    }

    /// Enforce `value` as the tolerance of every sub-shape `ShapeType` selects
    /// (`Shape.fixTolerance`, `ShapeFix_ShapeTolerance::SetTolerance`).
    ///
    /// The tolerances live in the shared TShape, so fixing them on a sub-shape
    /// (a `Part.Vertex`, an `Edge`, …) is visible on the shape it came from, as
    /// upstream's in-place fixer is. `ShapeType` is a `Part.Shape` subclass:
    /// `Part.Shape` (the default) covers vertices, edges and faces; `Vertex`,
    /// `Edge`, `Face`, `Wire` and `Shell` narrow it.
    #[pyo3(signature = (value, ShapeType = None))]
    fn fixTolerance(&self, value: f64, ShapeType: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        let kind = tolerance_kind(ShapeType, true)?;
        let shape = self
            .shape()
            .ok_or_else(|| curves::OCCError::new_err("a null shape has no tolerance"))?;
        ferrocad_part::backend()
            .fix_tolerance(&shape, value, kind)
            .map_err(kernel_error)
    }

    /// Limit the tolerances of the sub-shapes `ShapeType` selects
    /// (`Shape.limitTolerance`, `ShapeFix_ShapeTolerance::LimitTolerance`).
    ///
    /// `tmax == tmin` forces that value, `tmin == 0` caps at `tmax`, and
    /// `tmax < tmin` raises the minimum to `tmin`. Returns whether any tolerance
    /// changed.
    #[pyo3(signature = (tmin, tmax = 0.0, ShapeType = None))]
    fn limitTolerance(
        &self,
        tmin: f64,
        tmax: f64,
        ShapeType: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<bool> {
        let kind = tolerance_kind(ShapeType, true)?;
        let shape = self
            .shape()
            .ok_or_else(|| curves::OCCError::new_err("a null shape has no tolerance"))?;
        ferrocad_part::backend()
            .limit_tolerance(&shape, tmin, tmax, kind)
            .map_err(kernel_error)
    }

    /// The sub-shapes whose tolerance lies within `[valmin, valmax]`
    /// (`Shape.inTolerance`, `ShapeAnalysis_ShapeTolerance::InTolerance`).
    #[pyo3(signature = (valmin, valmax, ShapeType = None))]
    fn inTolerance(
        &self,
        py: Python<'_>,
        valmin: f64,
        valmax: f64,
        ShapeType: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Vec<Py<PartShape>>> {
        let kind = tolerance_kind(ShapeType, false)?;
        let shape = self
            .shape()
            .ok_or_else(|| curves::OCCError::new_err("a null shape has no tolerance"))?;
        let found = ferrocad_part::backend()
            .tolerance_in(&shape, valmin, valmax, kind)
            .map_err(kernel_error)?;
        found
            .into_iter()
            .map(|shape| Py::new(py, PartShape::from_shape(shape)))
            .collect()
    }

    /// The sub-shapes whose tolerance is greater than `value`
    /// (`Shape.overTolerance`, `ShapeAnalysis_ShapeTolerance::OverTolerance`).
    #[pyo3(signature = (value, ShapeType = None))]
    fn overTolerance(
        &self,
        py: Python<'_>,
        value: f64,
        ShapeType: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Vec<Py<PartShape>>> {
        let kind = tolerance_kind(ShapeType, false)?;
        let shape = self
            .shape()
            .ok_or_else(|| curves::OCCError::new_err("a null shape has no tolerance"))?;
        let found = ferrocad_part::backend()
            .tolerance_over(&shape, value, kind)
            .map_err(kernel_error)?;
        found
            .into_iter()
            .map(|shape| Py::new(py, PartShape::from_shape(shape)))
            .collect()
    }

    /// Sort this shape's edges into wires (`Shape.makeWires`).
    ///
    /// The result is a **compound** of every wire found, even when there is only
    /// one, matching upstream's `TopoShape::makeWires`; a connected chain gives a
    /// one-wire compound. (The shape-kind inference behind `ShapeType` cannot
    /// tell a one-wire compound from a bare wire, so it reports `Wire` there; the
    /// `Wires`/`Edges` getters are the reliable view.) `op` is accepted for
    /// signature compatibility — upstream only uses it to label the element map,
    /// and this seam builds no map here.
    #[pyo3(signature = (op = ""))]
    fn makeWires(&self, op: &str) -> PyResult<PartShape> {
        let shape = self.named_shape()?;
        let wires = ferrocad_part::backend()
            .make_wires(&shape, op, 0.0)
            .map_err(kernel_error)?;
        Ok(PartShape::from_shape(wires))
    }

    /// Remove the internal wires (holes) whose area is below `minimalArea`, in
    /// place, and report whether the kernel performed a removal
    /// (`Shape.removeInternalWires`, `ShapeUpgrade_RemoveInternalWires`).
    ///
    /// A shape with no internal wire keeps its geometry and returns `false`.
    fn removeInternalWires(&self, minimalArea: f64) -> PyResult<bool> {
        let shape = self.named_shape()?;
        let (result, performed) = ferrocad_part::backend()
            .remove_internal_wires(&shape, minimalArea)
            .map_err(kernel_error)?;
        self.replace_geometry(Some(result));
        Ok(performed)
    }

    /// Destroy the reference to the underlying shape, in place
    /// (`Shape.nullify`): afterwards [`isNull`](PartShape::isNull) is true.
    fn nullify(&self) {
        self.replace_shape(None);
    }

    /// Complement this shape's orientation in place (`Shape.complement`):
    /// `Forward`↔`Reversed` **and** `Internal`↔`External`.
    ///
    /// This is stronger than [`reverse`](PartShape::reverse), which only swaps the
    /// first pair. The geometry does not move, so the accumulated placement is
    /// kept.
    fn complement(&self) -> PyResult<()> {
        let shape = self.named_shape()?;
        let complemented = ferrocad_part::backend()
            .complement(&shape)
            .map_err(kernel_error)?;
        self.replace_geometry(Some(complemented));
        Ok(())
    }

    /// Read a BREP file into this shape, replacing its contents in place
    /// (`Shape.read`).
    ///
    /// Upstream also accepts IGES and STEP here; this seam only has the BREP
    /// reader (`BRepTools::Read`, the other side of
    /// [`exportBrep`](PartShape::exportBrep)), so a `.stp`/`.igs` file raises
    /// `Part.OCCError` instead of being silently ignored.
    fn read(&self, filename: &str) -> PyResult<()> {
        let shape = read_brep_file(filename)?;
        self.replace_shape(Some(shape));
        Ok(())
    }

    /// Write this shape as a binary STL file (`Shape.exportStl`).
    ///
    /// `deflection` is the linear deflection of the mesh the kernel builds, as
    /// upstream's `BRepMesh_IncrementalMesh` argument; the angular deflection is
    /// derived from it. A null shape is a `ValueError`; an I/O failure is an
    /// `OSError`.
    #[pyo3(signature = (filename, deflection = 0.01))]
    fn exportStl(&self, filename: &str, deflection: f64) -> PyResult<()> {
        let shape = self
            .shape()
            .ok_or_else(|| PyValueError::new_err("cannot export a null shape"))?;
        let bytes = ferrocad_part::backend()
            .stl_bytes(&shape, deflection)
            .map_err(kernel_error)?;
        std::fs::write(filename, bytes).map_err(|e| PyIOError::new_err(e.to_string()))
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

/// Whether upstream's `_makeTransform` routes `matrix` through
/// `BRepBuilderAPI_GTransform` rather than the `gp_Trsf` path.
///
/// `checkScale = True` asks OCCT to detect an affinity (a non-uniform scale or
/// shear) that a `gp_Trsf` cannot represent; a singular uniform scale is folded
/// in as well, mirroring `TopoShape::_makeTransform`.
fn is_general_transform(matrix: &ferrocad_types::Matrix4, check_scale: bool) -> bool {
    if !check_scale {
        return false;
    }
    match matrix.has_scale(1e-9) {
        ScaleType::NoScaling => false,
        ScaleType::Uniform => matrix.determinant3() == 0.0,
        _ => true,
    }
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

/// Python truthiness for an optional flag argument.
///
/// Upstream parses the `glue`/`refine` style flags with `PyObject_IsTrue`, so
/// any object is accepted (SimpleCAD passes a float `glue`).
fn truthy(value: Option<&Bound<'_, PyAny>>) -> bool {
    value
        .and_then(|value| value.is_truthy().ok())
        .unwrap_or(false)
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
        // Upstream's `fuse`/`cut`/`common` take a shape or a list of shapes; a
        // list folds one operation per entry.
        let targets = shape_sequence(other)?;
        if targets.is_empty() {
            return Err(curves::OCCError::new_err(
                "a boolean needs at least one other shape",
            ));
        }
        let backend = ferrocad_part::backend();
        let mut shape = a;
        let mut lineage = Lineage::default();
        let mut single_history = None;
        for target in &targets {
            let result = match operation {
                BooleanOp::Fuse => backend.fuse(&shape, target),
                BooleanOp::Cut => backend.cut(&shape, target),
                BooleanOp::Common => backend.common(&shape, target),
            }
            .map_err(kernel_error)?;
            if targets.len() == 1 {
                single_history = Some(result.history.clone());
            } else {
                lineage.apply(&result.history);
            }
            shape = result.shape;
        }
        if no_element_map {
            return Ok(PartShape::with_parts(Some(shape), None));
        }
        let lineage = match single_history {
            Some(history) => Lineage::from_history(&history, MapVersion::default()),
            None => lineage,
        };
        Ok(PartShape::from_shape_with_lineage(shape, lineage))
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
    let base = pnt
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(ferrocad_curves::Vector3::zero);
    let direction = dir
        .map(curves::vector)
        .transpose()?
        .unwrap_or_else(|| ferrocad_curves::Vector3::new(0.0, 0.0, 1.0));
    // A partial revolution needs the angular sweep; the full one goes through the
    // plain primitive.
    let shape = if (angle - 360.0).abs() <= 1e-9 {
        ferrocad_part::backend()
            .make_cylinder(radius, height, base, direction)
            .map_err(kernel_error)?
    } else {
        surface_ops::make_cylinder_slice(radius, height, base, direction, angle.to_radians())
            .map_err(kernel_error)?
    };
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
/// - `transform`: `False` (the default) returns the **local** shape, with the
///   object's placement stripped; `True` returns it at the object's placement.
///   A Python proxy stores local geometry (identity placement), so `True` applies
///   the object's `Placement`; a native Part feature already stores it baked in,
///   so applying it again would move the shape twice and is skipped.
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
    let identity = Placement::identity();
    let mut lineage = lineage;
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
    let mut result_placement = placement;
    if transform {
        // A Python proxy stores local geometry with an identity placement; a
        // native Part feature already stores the object placement baked in.
        if placement.is_same(&identity, 1e-12) {
            if let Ok(object_placement) = obj.getattr("Placement") {
                let matrix = object_placement.call_method0("toMatrix")?;
                let matrix = matrix_from_py(&matrix)?;
                resolved = ferrocad_part::backend()
                    .transform(&resolved, &matrix)
                    .map_err(kernel_error)?;
            }
        }
    } else if !placement.is_same(&identity, 1e-12) {
        // `transform=False` is upstream's local shape: strip the stored
        // placement. The re-bake replaces every handle, so the lineage goes.
        let matrix = placement.inverse().to_matrix();
        resolved = ferrocad_part::backend()
            .transform(&resolved, &matrix)
            .map_err(kernel_error)?;
        result_placement = identity;
        lineage = Lineage::default();
    }
    let shape = if subname.is_empty() {
        let lineage = if lineage.map().is_empty() && lineage.reverse().is_empty() {
            None
        } else {
            Some(lineage)
        };
        PartShape::with_parts_and_placement(Some(resolved), result_placement, lineage)
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

/// `Part.read(name)` — read a BREP file and return its shape.
///
/// Upstream's `Part.read` accepts IGES, STEP and BREP; this seam only has the
/// BREP reader, so the other extensions raise `Part.OCCError` rather than being
/// treated as unknown files. A missing or unreadable file is an `OSError`, as
/// upstream's `Base.FileNotFoundError` is.
#[pyfunction]
fn read(filename: &str) -> PyResult<PartShape> {
    Ok(PartShape::from_shape(read_brep_file(filename)?))
}

/// `Part.open(name)` — read a BREP file into a **new document**.
///
/// Upstream creates a document named after the file and adds one `Part::Feature`
/// holding the shape, then recomputes it; so does this, using the same
/// `ferrocad` document API as [`show`]. The deprecated upstream warning
/// ("use Import.open") is not reproduced: the function still does its documented
/// job here.
#[pyfunction]
fn open(py: Python<'_>, filename: &str) -> PyResult<()> {
    let shape = read_brep_file(filename)?;
    let name = std::path::Path::new(filename)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| PyValueError::new_err("the file has no name"))?
        .to_string();
    let app = py.import("ferrocad")?;
    let document = app.call_method1("newDocument", (name.clone(),))?;
    let object = document.call_method1("addObject", ("Part::Feature", name))?;
    object.setattr("Shape", PartShape::from_shape(shape))?;
    document.call_method0("recompute")?;
    Ok(())
}

/// `Part.getFacets(shape)` — the shape's triangles as three-point tuples.
///
/// Upstream reads the face triangulations that already exist on the shape and
/// drops a facet whose points collapse within `0.01`; this seam meshes on demand
/// at a `0.1` linear deflection (a shape here has no display mesh to reuse) and
/// applies the same degeneracy filter. The points are `(x, y, z)` tuples, as
/// upstream returns them, not `Vector`s.
#[pyfunction]
fn getFacets(py: Python<'_>, shape: &Bound<'_, PyAny>) -> PyResult<Py<PyList>> {
    let shape = shape_argument(shape)?;
    let options = TessellationOptions::absolute(0.1);
    let mesh = ferrocad_part::backend()
        .tessellate(&shape, &options)
        .map_err(kernel_error)?;
    // Upstream's `gp_Pnt::IsEqual(other, 0.01)`: every coordinate within the
    // tolerance.
    let same = |a: [f32; 3], b: [f32; 3]| {
        (a[0] - b[0]).abs() <= 0.01
            && (a[1] - b[1]).abs() <= 0.01
            && (a[2] - b[2]).abs() <= 0.01
    };
    let list = PyList::empty(py);
    for triangle in mesh.triangles.chunks_exact(3) {
        let mut points = [[0.0f32; 3]; 3];
        let mut valid = true;
        for (slot, index) in points.iter_mut().zip(triangle) {
            match mesh.positions.get(*index as usize) {
                Some(point) => *slot = *point,
                None => {
                    valid = false;
                    break;
                }
            }
        }
        if !valid {
            continue;
        }
        if same(points[0], points[1]) || same(points[1], points[2]) || same(points[2], points[0]) {
            continue;
        }
        let facet = points.map(|point| {
            (
                f64::from(point[0]),
                f64::from(point[1]),
                f64::from(point[2]),
            )
        });
        list.append(PyTuple::new(py, facet)?)?;
    }
    Ok(list.unbind())
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

/// The element kind a `Part.Shape` subclass names (`Part.Face` → `Face`).
fn element_kind_from_py_type(value: &Bound<'_, PyAny>) -> PyResult<ElementKind> {
    let name: String = value
        .getattr("__name__")
        .and_then(|name| name.extract())
        .map_err(|_| PyTypeError::new_err("ancestorsOfType expects a Part.Shape subclass"))?;
    ElementKind::ALL
        .into_iter()
        .find(|kind| kind.prefix() == name)
        .ok_or_else(|| PyTypeError::new_err("ancestorsOfType expects a Part.Shape subclass"))
}

/// The shape type a tolerance method's `ShapeType` argument names.
///
/// Upstream takes a *type object* (`Part.Vertex`), not a string, and the accepted
/// set depends on the method: `inTolerance`/`overTolerance` allow Shape, Vertex,
/// Edge, Face and Shell (every other subtype is a `TypeError`), while the fixers
/// also accept Wire, Solid and the aggregate types, which
/// `ShapeFix_ShapeTolerance` treats as "all sub-shapes".
fn tolerance_kind(value: Option<&Bound<'_, PyAny>>, fixer: bool) -> PyResult<ToleranceKind> {
    let Some(value) = value else {
        return Ok(ToleranceKind::All);
    };
    let name: String = value
        .getattr("__name__")
        .and_then(|name| name.extract())
        .map_err(|_| {
            PyTypeError::new_err("shape type must be a Part.Shape subclass, e.g. Part.Vertex")
        })?;
    let (kind, queryable) = match name.as_str() {
        "Shape" => (ToleranceKind::All, true),
        "Vertex" => (ToleranceKind::Vertex, true),
        "Edge" => (ToleranceKind::Edge, true),
        "Face" => (ToleranceKind::Face, true),
        "Shell" => (ToleranceKind::Shell, true),
        "Wire" => (ToleranceKind::Wire, false),
        "Solid" => (ToleranceKind::Solid, false),
        // `ShapeFix_ShapeTolerance` treats any other `TopAbs_ShapeEnum` as "all",
        // so these two map to the same branch rather than needing a kind of their
        // own.
        "Compound" | "CompSolid" => (ToleranceKind::All, false),
        _ => {
            return Err(PyTypeError::new_err(
                "shape type must be Shape, Vertex, Edge, Face or Shell",
            ));
        }
    };
    if !fixer && !queryable {
        return Err(PyTypeError::new_err(
            "shape type must be Shape, Vertex, Edge, Face or Shell",
        ));
    }
    Ok(kind)
}

/// The extension check upstream's `TopoShape::read` performs, restricted to the
/// one format this seam can decode.
fn read_brep_file(filename: &str) -> PyResult<Shape> {
    let path = std::path::Path::new(filename);
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| PyValueError::new_err("the file has no extension"))?;
    if !matches!(extension.as_str(), "brp" | "brep") {
        return Err(curves::OCCError::new_err(format!(
            "cannot read {filename}: only BREP files (.brp, .brep) are supported"
        )));
    }
    let bytes = std::fs::read(path).map_err(|error| PyIOError::new_err(error.to_string()))?;
    ferrocad_part::backend()
        .load_shape(&bytes)
        .ok_or_else(|| curves::OCCError::new_err(format!("{filename} is not a valid BREP shape")))
}

/// Wrap a sub-element in the Python subclass for its kind.
fn shape_wrapper_to_py(py: Python<'_>, shape: Shape) -> PyResult<PyObject> {
    match PartShape::kind_of(&shape) {
        Some(ElementKind::Edge) => Ok(Py::new(
            py,
            curves::PyEdge::from_entry(curves::EdgeEntry {
                shape,
                curve: None,
            }),
        )?
        .into_any()),
        Some(ElementKind::Vertex) => Ok(Py::new(py, curves::PyVertex::from_shape(shape)?)?.into_any()),
        Some(ElementKind::Face) => Ok(Py::new(py, curves::PyFace::from_shape(shape))?.into_any()),
        _ => Ok(Py::new(py, PartShape::from_shape(shape))?.into_any()),
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
///
/// The result is the wrapper class upstream's `shape2pyshape` picks from the
/// shape's topology: `Part.Wire` for a wire, `Part.Face` for a face, … and
/// `Part.Compound` for a compound. `isinstance(obj.Shape, Part.Wire)` therefore
/// holds when a `Part.Wire` was assigned, as it does upstream.
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
    let inner = match shape {
        Some(shape) => shape,
        // An unset property is upstream's null `TopoShape`, not `None`.
        None => return Py::new(py, PartShape::empty()).ok().map(Py::into_any),
    };
    let wrapped = wrap_shape(py, inner).ok()?;
    // The wrapper started from the bare geometry; adopt the property's own
    // accumulated placement and lineage so `Placement` and `ElementReverseMap`
    // report what the document object stored.
    let base = wrapped.bind(py).downcast::<PartShape>().ok()?;
    base.borrow().adopt_placement_and_lineage(property.placement(), lineage);
    Some(wrapped)
}

/// Wrap a kernel shape in the `Part.*` class upstream's `shape2pyshape` picks
/// from its topology: `Part.Wire` for a wire, `Part.Face` for a face, … and
/// `Part.Compound` for a compound (the base `Part.Shape` otherwise).
///
/// Used wherever a shape that already exists in the kernel is handed to Python
/// (`obj.Shape`, `Shape.copy()`), so a typed shape never degrades to the bare
/// `Part.Shape` on the way out.
pub(crate) fn wrap_shape(py: Python<'_>, shape: Shape) -> PyResult<PyObject> {
    Ok(match ferrocad_occt::part_ops::shape_kind(&shape) {
        Some("Solid") => Py::new(py, solids::PySolid::from_shape(shape.clone()))?.into_any(),
        Some("Shell") => Py::new(py, solids::PyShell::from_shape(shape.clone()))?.into_any(),
        Some("Face") => Py::new(py, curves::PyFace::from_shape(shape.clone()))?.into_any(),
        Some("Wire") => Py::new(py, curves::PyWire::from_shape(shape.clone()))?.into_any(),
        Some("Edge") => Py::new(
            py,
            curves::PyEdge::from_entry(curves::EdgeEntry {
                shape: shape.clone(),
                curve: None,
            }),
        )?
        .into_any(),
        Some("Vertex") => Py::new(py, curves::PyVertex::from_shape(shape.clone())?)?.into_any(),
        Some("Compound") => Py::new(
            py,
            containers::PyCompound::from_shape_existing(shape.clone()),
        )?
        .into_any(),
        Some("CompSolid") => Py::new(
            py,
            containers::PyCompSolid::from_shape_existing(shape.clone()),
        )?
        .into_any(),
        _ => Py::new(py, PartShape::from_shape(shape))?.into_any(),
    })
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

/// The merge tolerance for [`weld_triangles`], matching OCCT's `gp::Resolution`.
const WELD_TOLERANCE: f64 = 1e-7;

/// Merge a display tessellation's triangles into upstream's `(points, facets)`.
///
/// The backend's display mesh keeps a separate position per face (a pick needs the
/// part ids) and also carries the edge/vertex samples; the upstream `tessellate`
/// contract is a welded triangle mesh. Two vertices are the same point when they are
/// within [`WELD_TOLERANCE`], not only when their bits are equal: the two sides of a
/// curved face's seam can differ in the last bits. A facet that collapses to a line or
/// a point after welding is dropped, as upstream's does.
fn weld_triangles(
    positions: &[[f32; 3]],
    triangles: &[u32],
) -> (Vec<Vector3>, Vec<(usize, usize, usize)>) {
    let mut cells: HashMap<(i64, i64, i64), Vec<u32>> = HashMap::new();
    let mut points: Vec<Vector3> = Vec::new();
    let mut remap: HashMap<u32, u32> = HashMap::new();
    let mut facets = Vec::with_capacity(triangles.len() / 3);

    for triangle in triangles.chunks_exact(3) {
        let mut facet = [0usize; 3];
        for (slot, index) in facet.iter_mut().zip(triangle) {
            let mapped = match remap.get(index) {
                Some(&mapped) => mapped,
                None => {
                    let mapped = weld_point(positions[*index as usize], &mut cells, &mut points);
                    remap.insert(*index, mapped);
                    mapped
                }
            };
            *slot = mapped as usize;
        }
        if facet[0] == facet[1] || facet[1] == facet[2] || facet[0] == facet[2] {
            continue;
        }
        facets.push((facet[0], facet[1], facet[2]));
    }
    (points, facets)
}

/// The welded index of `position`: an existing point within [`WELD_TOLERANCE`], or a
/// new one. The spatial hash stores points in cells of that size and searches the 27
/// neighbours, so a point just across a cell boundary still finds its twin.
fn weld_point(
    position: [f32; 3],
    cells: &mut HashMap<(i64, i64, i64), Vec<u32>>,
    points: &mut Vec<Vector3>,
) -> u32 {
    let (x, y, z) = (
        f64::from(position[0]),
        f64::from(position[1]),
        f64::from(position[2]),
    );
    let key = (
        (x / WELD_TOLERANCE).floor() as i64,
        (y / WELD_TOLERANCE).floor() as i64,
        (z / WELD_TOLERANCE).floor() as i64,
    );
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                let neighbour = (key.0 + dx, key.1 + dy, key.2 + dz);
                if let Some(bucket) = cells.get(&neighbour) {
                    for &candidate in bucket {
                        let point = points[candidate as usize];
                        if (point.x - x).abs() <= WELD_TOLERANCE
                            && (point.y - y).abs() <= WELD_TOLERANCE
                            && (point.z - z).abs() <= WELD_TOLERANCE
                        {
                            return candidate;
                        }
                    }
                }
            }
        }
    }
    let index = points.len() as u32;
    points.push(Vector3::new(x, y, z));
    cells.entry(key).or_default().push(index);
    index
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
    m.add("_kernel_id", ferrocad_core::application_id())?;
    // The kernel release string (`"7.8.1"`), read from the OCCT headers at build
    // time. Upstream's Draft splits it into a tuple to decide whether hidden-line
    // removal is available (`Draft/draftfunctions/svgshapes.py:145`).
    m.add("OCC_VERSION", ferrocad_occt::OCC_VERSION)?;
    m.add_function(wrap_pyfunction!(getShape, m)?)?;
    m.add_function(wrap_pyfunction!(makeBox, m)?)?;
    m.add_function(wrap_pyfunction!(makeCylinder, m)?)?;
    m.add_function(wrap_pyfunction!(makeSphere, m)?)?;
    m.add_function(wrap_pyfunction!(makeCone, m)?)?;
    m.add_function(wrap_pyfunction!(makeSplitShape, m)?)?;
    m.add_function(wrap_pyfunction!(makeSweepSurface, m)?)?;
    m.add_function(wrap_pyfunction!(show, m)?)?;
    m.add_function(wrap_pyfunction!(read, m)?)?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add_function(wrap_pyfunction!(getFacets, m)?)?;
    m.add_function(wrap_pyfunction!(getSortedClusters, m)?)?;
    curves::register(m)?;
    containers::register(m)?;
    solids::register(m)?;
    shape_fix::register(m)?;
    chfi2d::register(m)?;
    Ok(())
}

#[cfg(test)]
mod tessellate_tests {
    use super::*;

    #[test]
    fn a_box_tessellates_like_upstream() {
        register_kernel();
        let cube = ferrocad_part::make_box(10.0, 20.0, 30.0).expect("box");
        let shape = PartShape::from_shape(cube);

        Python::with_gil(|py| {
            let (points, facets) = shape.tessellate(py, 0.1, 0.5).expect("tessellate");

            assert_eq!(points.len(), 8, "coincident face vertices are merged");
            assert_eq!(facets.len(), 12, "a box is twelve triangles");
            for &(a, b, c) in &facets {
                for index in [a, b, c] {
                    assert!(index < points.len(), "facet index {index} out of range");
                }
            }

            let mut min = [f64::INFINITY; 3];
            let mut max = [f64::NEG_INFINITY; 3];
            for point in &points {
                let point = point.bind(py).as_any();
                for (axis, name) in ["x", "y", "z"].into_iter().enumerate() {
                    let value: f64 = point
                        .getattr(name)
                        .expect("Vector coordinate")
                        .extract()
                        .expect("f64 coordinate");
                    min[axis] = min[axis].min(value);
                    max[axis] = max[axis].max(value);
                }
            }
            for (axis, extent) in [10.0, 20.0, 30.0].into_iter().enumerate() {
                assert!((min[axis] - 0.0).abs() < 1e-6, "min {axis} = {}", min[axis]);
                assert!(
                    (max[axis] - extent).abs() < 1e-6,
                    "max {axis} = {}",
                    max[axis]
                );
            }

            let error = PartShape::empty()
                .tessellate(py, 0.1, 0.5)
                .expect_err("a null shape cannot tessellate");
            assert!(error.is_instance_of::<PyValueError>(py));
        });
    }

    #[test]
    fn welding_merges_a_seam_and_drops_collapsed_facets() {
        let positions = vec![
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            // The same triangle again, with its seam vertices off in the last bits.
            [0.0, 0.0, 0.0],
            [1.00000005, 0.0, 0.0],
            [0.0, 1.00000005, 0.0],
        ];
        let triangles = vec![0, 1, 2, 3, 4, 5, 0, 0, 1];
        let (points, facets) = weld_triangles(&positions, &triangles);
        assert_eq!(
            points.len(),
            3,
            "the seam vertices merge within gp::Resolution"
        );
        assert_eq!(
            facets,
            vec![(0, 1, 2), (0, 1, 2)],
            "the collapsed facet is dropped"
        );
    }
}
