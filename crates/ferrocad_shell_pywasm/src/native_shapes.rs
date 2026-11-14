//! `Part`, `Mesh` and `MeshPart` for RustPython, over the OCCT kernel (B18b item 2).
//!
//! The desktop's `Part` is `ferrocad_part_py`, a PyO3 extension. `wasm32-unknown-unknown`
//! has no C ABI to load it with, so the browser's interpreter gets a **binding**, not a
//! second implementation: every class here wraps the same `ferrocad_geom::Shape` and calls
//! the same [`GeometryBackend`] the document half uses, so a script's box and a document's
//! `Part::Box` are one kernel, one shape type, one set of answers.
//!
//! The kernel is **OCCT**, injected once per process exactly as
//! `ferrocad_shell_rust` injects it for the document half. A host that composes only the
//! script half (a test) gets it registered here; the registration is idempotent, so the
//! browser's document half and this module never disagree about which backend is live.
//!
//! # The surface
//!
//! - `Part.makeBox` / `makeCylinder` / `makeSphere` / `makeCone` / `makeCompound`
//! - `Part.Wire(edges)` / `Part.Face(wires)` / `Part.makePolygon(points, closed=False)`
//! - `Part.Shape`: `Volume`, `Area`, `CenterOfMass`, `BoundBox`, `ShapeType`, `isValid()`,
//!   `isNull()`, the topology getters (`Faces`, `Edges`, `Vertexes`, `Solids`, `Shells`,
//!   `Wires`), the B-Rep explorer (`Surface`, `Curve`), `Placement`, `cut`/`fuse`/`common`,
//!   `translate`/`rotate`, `tessellate`
//! - `Part.Surface` / `Part.Curve`: the kernel's classification of a face or an edge, with
//!   the analytic frame (`Location`/`Axis`/`Radius`/`SemiAngle`/…) on the kinds that have
//!   it and `AttributeError` on the ones that do not, as upstream's per-kind classes do
//! - `Part.show(shape, name="Shape")`: a `Part::Feature` in the active document, which is
//!   the write that makes a standalone shape part of the model
//! - `Mesh.Mesh()` / `Mesh.Mesh(shape)`: points and facets, `Area`, `Volume`, `isSolid()`,
//!   `isValid()`, `BoundBox`, `Points`, `Facets`, `countPoints()`, `countFacets()`
//! - `MeshPart.meshFromShape(Shape=…, LinearDeflection=…, AngularDeflection=…)`
//!
//! Two cuts are stated rather than discovered: `Shape.Placement` is a value, not the
//! CPython binding's live view (`shape.Placement.Base.x = …` writes back there; here the
//! whole placement is assigned), and the interactive stage is not here (it is
//! `native_stage`). The geometry a script creates here is real: it is the kernel the
//! viewport draws.

use std::sync::{Arc, OnceLock};

use ferrocad_core::geometry::{Placement, Rotation, Vector3};
use ferrocad_curves::EdgeCurve;
use ferrocad_geom::{ElementKind, GeometryBackend, Shape, SurfaceData, TessellationOptions};
use ferrocad_occt::OcctBackend;
use rustpython_vm::builtins::PyStrRef;
use rustpython_vm::function::{FuncArgs, OptionalArg};
use rustpython_vm::types::Constructor;
use rustpython_vm::{
    Py, PyObjectRef, PyPayload, PyRef, PyResult, VirtualMachine, pyclass, pymodule,
};

pub(crate) use mesh::module_def as mesh_module_def;
pub(crate) use mesh_part::module_def as mesh_part_module_def;
pub(crate) use part::module_def as part_module_def;

/// The injected geometry backend, registering OCCT the first time any shape is built.
///
/// Idempotent: `ferrocad_part::register` keeps the first backend, so a host that already
/// registered its own (the browser's document half, every edition) keeps it.
fn kernel() -> Arc<dyn GeometryBackend> {
    static READY: OnceLock<()> = OnceLock::new();
    READY.get_or_init(|| ferrocad_part::register(Arc::new(OcctBackend::new())));
    ferrocad_part::backend()
}

/// The active document's handle, with the registry lock released.
pub(crate) fn active_handle() -> Result<ferrocad_core::DocumentHandle, String> {
    let registry = ferrocad_core::application()
        .lock()
        .map_err(|error| format!("the document registry is poisoned: {error}"))?;
    let name = match registry.active() {
        Some(name) => name.to_string(),
        None => return Err("no active document".to_string()),
    };
    registry
        .get(&name)
        .ok_or_else(|| format!("no document {name:?}"))
}

/// The `Shape` a Python value holds, when it is a `Part.Shape`.
pub(crate) fn shape_of(value: &PyObjectRef) -> Option<Shape> {
    value
        .downcast_ref::<part::PyShape>()
        .and_then(|shape| shape.resolved())
}

/// The B-Rep a document object holds, read from the registry.
pub(crate) fn object_shape(object: &str) -> Option<Shape> {
    let handle = active_handle().ok()?;
    let document = handle.lock().ok()?;
    let id = document.get_by_name(object)?;
    let property = document.object(id)?.properties.get("Shape")?;
    ferrocad_part::shape_of(property)?.shape()
}

/// The `Part.Shape` a document object holds, when the host has the registry.
///
/// This is what makes `document.getObject("Body").Shape` a real shape under RustPython
/// too: the payload carries the property's display text, but the kernel handle lives in
/// the registry the native module owns.
pub(crate) fn shape_py_of(object: &str, vm: &VirtualMachine) -> Option<PyObjectRef> {
    object_shape(object).map(|shape| part::shape_py(shape, vm))
}

/// How many faces the B-Rep a document object holds has, when it holds one.
///
/// `ViewObject.DiffuseColor` answers one colour per face, so it asks the same kernel the
/// `Faces` getter does rather than guessing.
pub(crate) fn object_face_count(object: &str) -> Option<usize> {
    object_shape(object).map(|shape| sub_shapes(&shape, ElementKind::Face).len())
}

/// A write the host cannot serve: a fixed-payload document, or no active one.
fn read_only_error(vm: &VirtualMachine, what: &str) -> rustpython_vm::builtins::PyBaseExceptionRef {
    vm.new_runtime_error(format!(
        "pywasm: {what} is the shell's; this host's document is read-only from Python"
    ))
}

/// Write `shape` into `object`'s `Shape` property (a script's `obj.Shape = …`).
///
/// This is the one typed write the doorway cannot carry: `ferrocad_doc_api` speaks editor
/// text, and a B-Rep is not text. The native module has `ferrocad_core` in process, so it
/// writes the property the way the CPython binding does.
pub(crate) fn set_object_shape(object: &str, shape: Shape) -> Result<(), String> {
    let handle = active_handle()?;
    let mut document = handle
        .lock()
        .map_err(|error| format!("the document is poisoned: {error}"))?;
    let id = document
        .get_by_name(object)
        .ok_or_else(|| format!("no object {object:?}"))?;
    if !document
        .object(id)
        .is_some_and(|object| object.properties.get("Shape").is_some())
    {
        return Err(format!("{object} has no Shape property"));
    }
    document.open_transaction_named(&format!("Set {object}.Shape"));
    if let Err(error) = document.set_property(
        id,
        "Shape",
        ferrocad_part::make_shape_property(ferrocad_part::ShapeProperty::new(shape)),
    ) {
        document.abort_transaction();
        return Err(error);
    }
    document.commit_transaction();
    let _ = document.recompute();
    Ok(())
}

/// Add `shape` to the active document as a `Part::Feature`, answering the object's name.
fn add_feature(shape: Shape, name: &str) -> Result<String, String> {
    let handle = active_handle()?;
    let mut document = handle
        .lock()
        .map_err(|error| format!("the document is poisoned: {error}"))?;
    let object = document.add_object(name, "Part::Feature");
    document.touch(object, false);
    document
        .set_property(
            object,
            "Shape",
            ferrocad_part::make_shape_property(ferrocad_part::ShapeProperty::new(shape)),
        )
        .map_err(|error| format!("Part::Feature: {error}"))?;
    let _ = document.recompute();
    Ok(document
        .object(object)
        .map(|object| object.name.clone())
        .unwrap_or_default())
}

/// The sub-shapes of one kind, in the kernel's order (`Face1`, `Face2`, …).
fn sub_shapes(shape: &Shape, kind: ElementKind) -> Vec<Shape> {
    kernel()
        .elements(shape, kind)
        .into_iter()
        .map(|element| element.shape)
        .collect()
}

/// A shape's display tessellation.
fn tessellate(
    shape: &Shape,
    linear: f64,
    angular: f64,
) -> Result<ferrocad_geom::Tessellation, String> {
    let options = TessellationOptions {
        linear_deflection: Some(linear),
        angular_deflection: angular,
        ..TessellationOptions::default()
    };
    kernel()
        .tessellate(shape, &options)
        .map_err(|error| error.to_string())
}

/// A triangle mesh from a shape's display tessellation.
///
/// The triangles are welded by coordinate (`from_facet_points`), because the kernel
/// tessellates each face on its own: an unwelded box would be a triangle soup, not the
/// closed mesh a script means by "the mesh of this solid".
fn mesh_from_shape(
    shape: &Shape,
    linear: f64,
    angular: f64,
) -> Result<ferrocad_mesh::Mesh, String> {
    let tessellation = tessellate(shape, linear, angular)?;
    let point = |index: u32| -> Vector3 {
        tessellation
            .positions
            .get(index as usize)
            .map(|[x, y, z]| Vector3::new(*x as f64, *y as f64, *z as f64))
            .unwrap_or_else(Vector3::zero)
    };
    let triangles: Vec<[Vector3; 3]> = tessellation
        .triangles
        .chunks_exact(3)
        .map(|triangle| [point(triangle[0]), point(triangle[1]), point(triangle[2])])
        .collect();
    ferrocad_mesh::Mesh::from_facet_points(&triangles).map_err(|error| format!("{error:?}"))
}

#[pymodule(name = "Part")]
mod part {
    #![allow(non_snake_case, non_upper_case_globals)]

    use std::sync::{Arc, Mutex};

    use super::{
        EdgeCurve, ElementKind, OptionalArg, Placement, Py, PyObjectRef, PyPayload, PyRef, PyResult,
        PyStrRef, Rotation, Shape, SurfaceData, Vector3, VirtualMachine, kernel, pyclass, sub_shapes,
        tessellate,
    };
    use rustpython_vm::types::Representable;

    #[pyattr]
    const __version__: &str = env!("CARGO_PKG_VERSION");

    /// The kernel this binding was composed with (the same id the document half reports).
    #[pyattr]
    const _kernel_id: &str = "occt";

    /// A B-Rep shape: the kernel's handle, shared exactly as `ferrocad_part_py` shares it.
    #[pyattr]
    #[pyclass(name = "Shape", module = "Part")]
    #[derive(Debug, PyPayload, Clone)]
    pub struct PyShape {
        inner: Arc<Mutex<Option<Shape>>>,
        /// The placement a script assigned (`Shape.Placement = …`); `None` is identity.
        ///
        /// The geometry stays baked — `translate`/`rotate` still return moved shapes — so
        /// this is the outer transform a *later* assignment put on top, applied when the
        /// shape is read. `None` is the common case and costs nothing.
        placement: Arc<Mutex<Option<Placement>>>,
    }

    impl PyShape {
        fn of(shape: Shape) -> Self {
            Self {
                inner: Arc::new(Mutex::new(Some(shape))),
                placement: Arc::new(Mutex::new(None)),
            }
        }

        /// The shape at its placement, or `None` for a null handle.
        pub(super) fn resolved(&self) -> Option<Shape> {
            let base = self.inner.lock().ok().and_then(|slot| slot.clone())?;
            let placement = *self.placement.lock().ok()?;
            match placement {
                Some(placement) => kernel().moved(&base, &placement).ok(),
                None => Some(base),
            }
        }

        fn shape(&self, vm: &VirtualMachine) -> PyResult<Shape> {
            self.resolved()
                .ok_or_else(|| vm.new_runtime_error("the shape is null"))
        }
    }

    pub(super) fn shape_py(shape: Shape, vm: &VirtualMachine) -> PyObjectRef {
        PyShape::of(shape).into_ref(&vm.ctx).into()
    }

    fn geometry_error(
        vm: &VirtualMachine,
        error: ferrocad_geom::GeomError,
    ) -> rustpython_vm::builtins::PyBaseExceptionRef {
        vm.new_value_error(error.to_string())
    }

    fn vector_arg(value: &PyObjectRef, vm: &VirtualMachine) -> PyResult<Vector3> {
        crate::native::vector_of(value)
            .ok_or_else(|| vm.new_type_error("expected a FreeCAD.Vector"))
    }

    /// A Python number as `f64`. `int` is accepted, which `f64::try_from_object` is not.
    fn number(value: &PyObjectRef, label: &str, vm: &VirtualMachine) -> PyResult<f64> {
        value
            .try_float(vm)
            .map(|value| value.to_f64())
            .map_err(|_| vm.new_type_error(format!("{label} must be a number")))
    }

    fn list_of_shapes(shapes: Vec<Shape>, vm: &VirtualMachine) -> PyObjectRef {
        let items: Vec<PyObjectRef> = shapes
            .into_iter()
            .map(|shape| shape_py(shape, vm))
            .collect();
        vm.ctx.new_list(items).into()
    }

    /// `Part.makeBox(length, width, height)`.
    #[pyfunction]
    pub fn makeBox(
        length: PyObjectRef,
        width: PyObjectRef,
        height: PyObjectRef,
        vm: &VirtualMachine,
    ) -> PyResult<PyObjectRef> {
        let shape = kernel()
            .make_box(
                number(&length, "length", vm)?,
                number(&width, "width", vm)?,
                number(&height, "height", vm)?,
            )
            .map_err(|error| geometry_error(vm, error))?;
        Ok(shape_py(shape, vm))
    }

    /// `Part.makeCylinder(radius, height, angle=360)`.
    #[pyfunction]
    pub fn makeCylinder(
        radius: PyObjectRef,
        height: PyObjectRef,
        angle: OptionalArg<PyObjectRef>,
        vm: &VirtualMachine,
    ) -> PyResult<PyObjectRef> {
        // The full-angle cylinder is the backend's; a partial one needs a kernel feature
        // this slice does not bind, so it is refused rather than silently made full.
        if let Some(angle) = angle.into_option() {
            if (number(&angle, "angle", vm)? - 360.0).abs() > 1e-9 {
                return Err(vm.new_value_error(
                    "Part.makeCylinder: only a full 360° cylinder is bound in this build",
                ));
            }
        }
        let shape = kernel()
            .make_cylinder(
                number(&radius, "radius", vm)?,
                number(&height, "height", vm)?,
                Vector3::zero(),
                Vector3::new(0.0, 0.0, 1.0),
            )
            .map_err(|error| geometry_error(vm, error))?;
        Ok(shape_py(shape, vm))
    }

    /// `Part.makeSphere(radius)`.
    #[pyfunction]
    pub fn makeSphere(radius: PyObjectRef, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
        let shape = kernel()
            .make_sphere(number(&radius, "radius", vm)?, Vector3::zero())
            .map_err(|error| geometry_error(vm, error))?;
        Ok(shape_py(shape, vm))
    }

    /// `Part.makeCone(radius1, radius2, height)`.
    #[pyfunction]
    pub fn makeCone(
        radius1: PyObjectRef,
        radius2: PyObjectRef,
        height: PyObjectRef,
        vm: &VirtualMachine,
    ) -> PyResult<PyObjectRef> {
        let shape = kernel()
            .make_cone(
                number(&radius1, "radius1", vm)?,
                number(&radius2, "radius2", vm)?,
                number(&height, "height", vm)?,
                Vector3::zero(),
                Vector3::new(0.0, 0.0, 1.0),
            )
            .map_err(|error| geometry_error(vm, error))?;
        Ok(shape_py(shape, vm))
    }

    /// `Part.makeCompound([shape, …])`.
    #[pyfunction]
    pub fn makeCompound(shapes: PyObjectRef, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
        let shapes: Vec<Shape> = shapes
            .try_to_value::<Vec<PyObjectRef>>(vm)?
            .iter()
            .filter_map(super::shape_of)
            .collect();
        if shapes.is_empty() {
            return Err(vm.new_type_error("Part.makeCompound expects Part.Shape objects"));
        }
        let compound = kernel()
            .make_compound(&shapes)
            .map_err(|error| geometry_error(vm, error))?;
        Ok(shape_py(compound, vm))
    }

    /// The shapes a Python sequence (or a single shape) holds.
    fn shapes_arg(
        value: &PyObjectRef,
        label: &str,
        vm: &VirtualMachine,
    ) -> PyResult<Vec<Shape>> {
        if let Some(shape) = super::shape_of(value) {
            return Ok(vec![shape]);
        }
        let items: Vec<PyObjectRef> = value
            .clone()
            .try_into_value(vm)
            .map_err(|_| vm.new_type_error(format!("{label} expects Part.Shape objects")))?;
        items
            .iter()
            .map(|item| {
                super::shape_of(item)
                    .ok_or_else(|| vm.new_type_error(format!("{label} expects Part.Shape objects")))
            })
            .collect()
    }

    /// `Part.makePolygon(points, closed=False)`: a wire through the points.
    ///
    /// The segments are `ferrocad_curves::polygon`'s — the same ones
    /// `ferrocad_part_py` builds — so the two runtimes make the same wire.
    #[pyfunction]
    pub fn makePolygon(
        points: PyObjectRef,
        closed: OptionalArg<bool>,
        vm: &VirtualMachine,
    ) -> PyResult<PyObjectRef> {
        let items: Vec<PyObjectRef> = points
            .clone()
            .try_into_value(vm)
            .map_err(|_| vm.new_type_error("makePolygon expects a list of Vector points"))?;
        let mut parsed = Vec::with_capacity(items.len());
        for item in &items {
            parsed.push(vector_arg(item, vm)?);
        }
        let curves = ferrocad_curves::polygon(&parsed, closed.into_option().unwrap_or(false))
            .map_err(|error| vm.new_value_error(error.to_string()))?;
        let mut edges = Vec::with_capacity(curves.len());
        for curve in &curves {
            edges.push(
                kernel()
                    .make_edge(curve)
                    .map_err(|error| geometry_error(vm, error))?,
            );
        }
        let wire = kernel()
            .make_wire(&edges)
            .map_err(|error| geometry_error(vm, error))?;
        Ok(shape_py(wire, vm))
    }

    /// `Part.Wire(edges)`: the wire those edges form.
    ///
    /// CPython's `Part.Wire` is a `PartShape` subclass; here a shape is one class
    /// (`Part.Shape`, whose `ShapeType` says `Wire`), so this is the constructor as a
    /// function and the result carries the whole `Shape` surface.
    #[pyfunction]
    pub fn Wire(edges: PyObjectRef, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
        let edges = shapes_arg(&edges, "Part.Wire", vm)?;
        if edges.is_empty() {
            return Err(vm.new_value_error("Part.Wire expects at least one edge"));
        }
        let wire = kernel()
            .make_wire(&edges)
            .map_err(|error| geometry_error(vm, error))?;
        Ok(shape_py(wire, vm))
    }

    /// `Part.Face(wires)`: the first wire is the outline, the rest are holes.
    #[pyfunction]
    pub fn Face(wires: PyObjectRef, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
        let wires = shapes_arg(&wires, "Part.Face", vm)?;
        if wires.is_empty() {
            return Err(vm.new_value_error("Part.Face expects at least one wire"));
        }
        let face = kernel()
            .make_face(&wires)
            .map_err(|error| geometry_error(vm, error))?;
        Ok(shape_py(face, vm))
    }

    /// `Part.show(shape, name="Shape")`: add the shape to the active document.
    #[pyfunction]
    pub fn show(
        shape: PyRef<PyShape>,
        name: OptionalArg<PyStrRef>,
        vm: &VirtualMachine,
    ) -> PyResult<PyObjectRef> {
        if !crate::document_writes() {
            return Err(super::read_only_error(vm, "adding a shape to the document"));
        }
        let shape = shape.shape(vm)?;
        let name = name
            .into_option()
            .map(|name| name.to_str().unwrap_or_default().to_string())
            .unwrap_or_else(|| "Shape".to_string());
        let object = super::add_feature(shape, &name).map_err(|error| vm.new_value_error(error))?;
        Ok(crate::native::object_named_py(&object, vm).unwrap_or_else(|| vm.ctx.none()))
    }

    #[pyclass]
    impl PyShape {
        /// The solid volume; `0` when the shape encloses nothing.
        #[pygetset]
        fn Volume(zelf: &Py<Self>) -> f64 {
            zelf.resolved()
                .and_then(|shape| kernel().mass_properties(&shape))
                .map(|properties| properties.volume)
                .unwrap_or(0.0)
        }

        /// The surface area.
        #[pygetset]
        fn Area(zelf: &Py<Self>) -> f64 {
            zelf.resolved()
                .and_then(|shape| kernel().mass_properties(&shape))
                .map(|properties| properties.area)
                .unwrap_or(0.0)
        }

        /// The centre of mass as a `FreeCAD.Vector`.
        #[pygetset]
        fn CenterOfMass(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            match zelf
                .resolved()
                .and_then(|shape| kernel().mass_properties(&shape))
            {
                Some(properties) => crate::native::vector_py(properties.center_of_mass, vm),
                None => vm.ctx.none(),
            }
        }

        /// The axis-aligned bounds as a `FreeCAD.BoundBox`.
        #[pygetset]
        fn BoundBox(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            let (min, max) = zelf
                .resolved()
                .and_then(|shape| kernel().bounds(&shape))
                .unwrap_or((Vector3::zero(), Vector3::zero()));
            crate::native::bound_box_py(min, max, vm)
        }

        /// The kernel's topological kind (`Solid`, `Face`, `Compound`, …).
        #[pygetset]
        fn ShapeType(zelf: &Py<Self>) -> String {
            zelf.resolved()
                .and_then(|shape| ferrocad_occt::part_ops::shape_kind(&shape))
                .unwrap_or("Compound")
                .to_string()
        }

        #[pygetset]
        fn Faces(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            match zelf.resolved() {
                Some(shape) => list_of_shapes(sub_shapes(&shape, ElementKind::Face), vm),
                None => vm.ctx.new_list(Vec::new()).into(),
            }
        }

        #[pygetset]
        fn Edges(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            match zelf.resolved() {
                Some(shape) => list_of_shapes(sub_shapes(&shape, ElementKind::Edge), vm),
                None => vm.ctx.new_list(Vec::new()).into(),
            }
        }

        #[pygetset]
        fn Vertexes(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            match zelf.resolved() {
                Some(shape) => list_of_shapes(sub_shapes(&shape, ElementKind::Vertex), vm),
                None => vm.ctx.new_list(Vec::new()).into(),
            }
        }

        #[pygetset]
        fn Solids(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            match zelf.resolved() {
                Some(shape) => list_of_shapes(sub_shapes(&shape, ElementKind::Solid), vm),
                None => vm.ctx.new_list(Vec::new()).into(),
            }
        }

        #[pygetset]
        fn Shells(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            match zelf.resolved() {
                Some(shape) => list_of_shapes(sub_shapes(&shape, ElementKind::Shell), vm),
                None => vm.ctx.new_list(Vec::new()).into(),
            }
        }

        #[pygetset]
        fn Wires(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            match zelf.resolved() {
                Some(shape) => list_of_shapes(sub_shapes(&shape, ElementKind::Wire), vm),
                None => vm.ctx.new_list(Vec::new()).into(),
            }
        }

        /// The face's backing surface (`Face.Surface`), when the kernel classifies it.
        ///
        /// A plane, cylinder, sphere, cone or torus reports its elementary frame; any
        /// other surface keeps the kernel's type id (`Part::GeomBSplineSurface`), which is
        /// what a B-Rep explorer reads.
        #[pygetset]
        fn Surface(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let shape = zelf.shape(vm)?;
            Ok(match kernel().surface_data(&shape) {
                Some(data) => surface_py(data, vm),
                None => vm.ctx.none(),
            })
        }

        /// The analytic curve behind an edge (`Edge.Curve`), when the kernel classifies
        /// it: a segment's edge reports a `Line`, a trimmed circular edge a `Circle`, as
        /// upstream's basis-curve rule does.
        #[pygetset]
        fn Curve(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let shape = zelf.shape(vm)?;
            Ok(match kernel().curve_of(&shape) {
                Ok(curve) => curve_py(curve, vm),
                Err(_) => vm.ctx.none(),
            })
        }

        /// The placement assigned to this shape (`Shape.Placement`).
        ///
        /// The geometry is baked, so this is the outer transform: reading it after
        /// `translate`/`rotate` gives identity (the move is in the geometry), and a
        /// shape it is assigned to reports that placement and draws where it says. The
        /// CPython binding keeps a live view (`shape.Placement.Base.x = …` writes back);
        /// here a placement is a value, so assign the whole one.
        #[pygetset]
        fn Placement(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            let placement = zelf
                .placement
                .lock()
                .ok()
                .and_then(|slot| *slot)
                .unwrap_or_else(Placement::identity);
            crate::native::placement_py(placement, vm)
        }

        #[pygetset(setter)]
        fn set_Placement(zelf: &Py<Self>, value: PyObjectRef, vm: &VirtualMachine) -> PyResult<()> {
            let placement = crate::native::placement_of(&value)
                .ok_or_else(|| vm.new_type_error("Shape.Placement needs a FreeCAD.Placement"))?;
            let mut slot = zelf
                .placement
                .lock()
                .map_err(|_| vm.new_runtime_error("the placement is busy"))?;
            *slot = Some(placement);
            Ok(())
        }

        /// Whether the kernel considers the B-Rep valid.
        #[pymethod]
        fn isValid(zelf: &Py<Self>) -> bool {
            zelf.resolved()
                .map(|shape| kernel().is_valid(&shape))
                .unwrap_or(false)
        }

        /// Whether this is the null shape.
        #[pymethod]
        fn isNull(zelf: &Py<Self>) -> bool {
            zelf.resolved().is_none()
        }

        #[pymethod]
        fn cut(
            zelf: &Py<Self>,
            other: PyRef<PyShape>,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            let (left, right) = (zelf.shape(vm)?, other.shape(vm)?);
            let result = kernel()
                .cut(&left, &right, 0.0)
                .map_err(|error| geometry_error(vm, error))?;
            Ok(shape_py(result.shape, vm))
        }

        #[pymethod]
        fn fuse(
            zelf: &Py<Self>,
            other: PyRef<PyShape>,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            let (left, right) = (zelf.shape(vm)?, other.shape(vm)?);
            let result = kernel()
                .fuse(&left, &right, 0.0)
                .map_err(|error| geometry_error(vm, error))?;
            Ok(shape_py(result.shape, vm))
        }

        #[pymethod]
        fn common(
            zelf: &Py<Self>,
            other: PyRef<PyShape>,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            let (left, right) = (zelf.shape(vm)?, other.shape(vm)?);
            let result = kernel()
                .common(&left, &right, 0.0)
                .map_err(|error| geometry_error(vm, error))?;
            Ok(shape_py(result.shape, vm))
        }

        /// Translate by `vector`, baking the move into the returned shape.
        #[pymethod]
        fn translate(
            zelf: &Py<Self>,
            vector: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            let shape = zelf.shape(vm)?;
            let offset = vector_arg(&vector, vm)?;
            let placed = Placement::new(offset, Rotation::identity());
            let moved = kernel()
                .moved(&shape, &placed)
                .map_err(|error| geometry_error(vm, error))?;
            Ok(shape_py(moved, vm))
        }

        /// Rotate about `center` by `angle` degrees around `axis`, baked into the result.
        #[pymethod]
        fn rotate(
            zelf: &Py<Self>,
            center: PyObjectRef,
            axis: PyObjectRef,
            angle: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            let shape = zelf.shape(vm)?;
            let center = vector_arg(&center, vm)?;
            let axis = vector_arg(&axis, vm)?;
            let degrees = number(&angle, "angle", vm)?;
            let rotation = Rotation::from_axis_angle(&axis, degrees.to_radians());
            let moved_center = rotation.to_matrix().transform(&center);
            let base = Vector3::new(
                center.x - moved_center.x,
                center.y - moved_center.y,
                center.z - moved_center.z,
            );
            let placed = Placement::new(base, rotation);
            let moved = kernel()
                .moved(&shape, &placed)
                .map_err(|error| geometry_error(vm, error))?;
            Ok(shape_py(moved, vm))
        }

        /// `(vertices, facets)`: positions as `FreeCAD.Vector`s and index triples.
        #[pymethod]
        fn tessellate(
            zelf: &Py<Self>,
            deflection: OptionalArg<PyObjectRef>,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            let shape = zelf.shape(vm)?;
            let linear = match deflection.into_option() {
                Some(value) => value.try_float(vm)?.to_f64(),
                None => 0.1,
            };
            let tessellation =
                tessellate(&shape, linear, 0.5).map_err(|error| vm.new_value_error(error))?;
            let points: Vec<PyObjectRef> = tessellation
                .positions
                .iter()
                .map(|[x, y, z]| {
                    crate::native::vector_py(Vector3::new(*x as f64, *y as f64, *z as f64), vm)
                })
                .collect();
            let facets: Vec<PyObjectRef> = tessellation
                .triangles
                .chunks_exact(3)
                .map(|triangle| {
                    vm.ctx
                        .new_tuple(vec![
                            vm.new_pyobj(triangle[0] as i64),
                            vm.new_pyobj(triangle[1] as i64),
                            vm.new_pyobj(triangle[2] as i64),
                        ])
                        .into()
                })
                .collect();
            Ok(vm
                .ctx
                .new_tuple(vec![
                    vm.ctx.new_list(points).into(),
                    vm.ctx.new_list(facets).into(),
                ])
                .into())
        }
    }

    /// A face's backing surface (`Face.Surface`), as the kernel classified it.
    ///
    /// The CPython binding has one class per elementary surface (`Part.Plane`,
    /// `Part.Cylinder`, …); this is one class whose members exist only on the kinds that
    /// have them, so `surface.Radius` raises `AttributeError` on a plane exactly as
    /// upstream's class split does. `TypeId` is the same registered name either way.
    #[pyattr]
    #[pyclass(name = "Surface", module = "Part")]
    #[derive(Debug, PyPayload, Clone)]
    pub struct PySurface {
        data: SurfaceData,
    }

    impl PySurface {
        /// The upstream class name (`"Plane"`, `"Cylinder"`, …).
        fn kind(&self) -> &str {
            match &self.data {
                SurfaceData::Plane { .. } => "Plane",
                SurfaceData::Cylinder { .. } => "Cylinder",
                SurfaceData::Sphere { .. } => "Sphere",
                SurfaceData::Cone { .. } => "Cone",
                SurfaceData::Torus { .. } => "Toroid",
                SurfaceData::Other { type_id } => {
                    type_id.rsplit("::").next().unwrap_or(type_id)
                }
            }
        }
    }

    fn surface_py(data: SurfaceData, vm: &VirtualMachine) -> PyObjectRef {
        PySurface { data }.into_ref(&vm.ctx).into()
    }

    #[pyclass(with(Representable))]
    impl PySurface {
        /// The registered type name the kernel reports (`Part::GeomPlane`).
        #[pygetset]
        fn TypeId(zelf: &Py<Self>) -> String {
            match &zelf.data {
                SurfaceData::Plane { .. } => "Part::GeomPlane".to_string(),
                SurfaceData::Cylinder { .. } => "Part::GeomCylinder".to_string(),
                SurfaceData::Sphere { .. } => "Part::GeomSphere".to_string(),
                SurfaceData::Cone { .. } => "Part::GeomCone".to_string(),
                SurfaceData::Torus { .. } => "Part::GeomToroid".to_string(),
                SurfaceData::Other { type_id } => type_id.clone(),
            }
        }

        /// The origin (a plane) or the centre (a curved surface).
        #[pygetset]
        fn Location(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let point = match &zelf.data {
                SurfaceData::Plane { origin, .. }
                | SurfaceData::Cylinder { origin, .. }
                | SurfaceData::Cone { origin, .. } => *origin,
                SurfaceData::Sphere { center, .. } | SurfaceData::Torus { center, .. } => *center,
                SurfaceData::Other { .. } => return Err(no_member(vm, zelf.kind(), "Location")),
            };
            Ok(crate::native::vector_py(point, vm))
        }

        /// The plane's origin, as upstream's `Plane.Position` reports it.
        #[pygetset]
        fn Position(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            match &zelf.data {
                SurfaceData::Plane { origin, .. } => Ok(crate::native::vector_py(*origin, vm)),
                _ => Err(no_member(vm, zelf.kind(), "Position")),
            }
        }

        /// The normal of a plane, or the axis of a surface of revolution.
        #[pygetset]
        fn Axis(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let axis = match &zelf.data {
                SurfaceData::Plane { normal, .. } => *normal,
                SurfaceData::Cylinder { axis, .. }
                | SurfaceData::Sphere { axis, .. }
                | SurfaceData::Cone { axis, .. }
                | SurfaceData::Torus { axis, .. } => *axis,
                SurfaceData::Other { .. } => return Err(no_member(vm, zelf.kind(), "Axis")),
            };
            Ok(crate::native::vector_py(axis, vm))
        }

        /// The centre of a cylinder, sphere, cone or torus (a plane has `Location`).
        #[pygetset]
        fn Center(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let point = match &zelf.data {
                SurfaceData::Cylinder { origin, .. } | SurfaceData::Cone { origin, .. } => *origin,
                SurfaceData::Sphere { center, .. } | SurfaceData::Torus { center, .. } => *center,
                _ => return Err(no_member(vm, zelf.kind(), "Center")),
            };
            Ok(crate::native::vector_py(point, vm))
        }

        /// The radius of a cylinder, sphere or cone.
        #[pygetset]
        fn Radius(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<f64> {
            match &zelf.data {
                SurfaceData::Cylinder { radius, .. }
                | SurfaceData::Sphere { radius, .. }
                | SurfaceData::Cone { radius, .. } => Ok(*radius),
                _ => Err(no_member(vm, zelf.kind(), "Radius")),
            }
        }

        /// A cone's semi-angle, in radians (upstream's `Cone.SemiAngle`).
        #[pygetset]
        fn SemiAngle(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<f64> {
            match &zelf.data {
                SurfaceData::Cone { semi_angle, .. } => Ok(*semi_angle),
                _ => Err(no_member(vm, zelf.kind(), "SemiAngle")),
            }
        }

        /// A torus's major radius.
        #[pygetset]
        fn MajorRadius(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<f64> {
            match &zelf.data {
                SurfaceData::Torus { major_radius, .. } => Ok(*major_radius),
                _ => Err(no_member(vm, zelf.kind(), "MajorRadius")),
            }
        }

        /// A torus's minor radius.
        #[pygetset]
        fn MinorRadius(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<f64> {
            match &zelf.data {
                SurfaceData::Torus { minor_radius, .. } => Ok(*minor_radius),
                _ => Err(no_member(vm, zelf.kind(), "MinorRadius")),
            }
        }
    }

    impl Representable for PySurface {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            Ok(format!("<{} object>", zelf.kind()))
        }
    }

    /// An edge's analytic curve (`Edge.Curve`), as the kernel classified it.
    ///
    /// The members exist on the kinds that have them: `StartPoint`/`EndPoint` for a
    /// bounded curve, `Center`/`Axis` for a conic, `Radius` for a circle, the two radii
    /// for an ellipse. Anything else (a B-spline, an offset) still answers `TypeId` and
    /// the evaluator (`value`/`tangent`).
    #[pyattr]
    #[pyclass(name = "Curve", module = "Part")]
    #[derive(Debug, PyPayload, Clone)]
    pub struct PyCurve {
        curve: EdgeCurve,
    }

    fn curve_py(curve: EdgeCurve, vm: &VirtualMachine) -> PyObjectRef {
        PyCurve { curve }.into_ref(&vm.ctx).into()
    }

    /// A member this kind of `Surface`/`Curve` does not have.
    ///
    /// `AttributeError` is the right answer, not `None`: upstream has one class per kind,
    /// so `hasattr(plane, "Radius")` is `False` there and must be here too.
    fn no_member(
        vm: &VirtualMachine,
        kind: &str,
        member: &str,
    ) -> rustpython_vm::builtins::PyBaseExceptionRef {
        vm.new_attribute_error(format!("a {kind} has no {member}"))
    }

    #[pyclass(with(Representable))]
    impl PyCurve {
        /// The registered type name the kernel reports (`Part::GeomCircle`).
        #[pygetset]
        fn TypeId(zelf: &Py<Self>) -> String {
            format!("Part::Geom{}", zelf.curve.kind())
        }

        /// The start point of a bounded curve.
        #[pygetset]
        fn StartPoint(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            match zelf.curve.endpoints() {
                Some((start, _)) => Ok(crate::native::vector_py(start, vm)),
                None => Err(no_member(vm, zelf.curve.kind(), "StartPoint")),
            }
        }

        /// The end point of a bounded curve.
        #[pygetset]
        fn EndPoint(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            match zelf.curve.endpoints() {
                Some((_, end)) => Ok(crate::native::vector_py(end, vm)),
                None => Err(no_member(vm, zelf.curve.kind(), "EndPoint")),
            }
        }

        /// The curve's parameter range, `(first, last)`, when it is bounded.
        #[pygetset]
        fn ParameterRange(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            match zelf.curve.parameter_range() {
                Some((first, last)) => Ok(vm
                    .ctx
                    .new_tuple(vec![vm.new_pyobj(first), vm.new_pyobj(last)])
                    .into()),
                None => Err(no_member(vm, zelf.curve.kind(), "ParameterRange")),
            }
        }

        /// The centre of a circle or an ellipse.
        #[pygetset]
        fn Center(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let center = match &zelf.curve {
                EdgeCurve::Circle(circle) => circle.center(),
                EdgeCurve::Arc(arc) => arc.circle().center(),
                EdgeCurve::Ellipse(ellipse) => ellipse.center(),
                EdgeCurve::EllipticalArc(arc) => arc.ellipse().center(),
                _ => return Err(no_member(vm, zelf.curve.kind(), "Center")),
            };
            Ok(crate::native::vector_py(center, vm))
        }

        /// The plane normal of a circle or an ellipse.
        #[pygetset]
        fn Axis(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let axis = match &zelf.curve {
                EdgeCurve::Circle(circle) => circle.normal(),
                EdgeCurve::Arc(arc) => arc.circle().normal(),
                EdgeCurve::Ellipse(ellipse) => ellipse.normal(),
                EdgeCurve::EllipticalArc(arc) => arc.ellipse().normal(),
                _ => return Err(no_member(vm, zelf.curve.kind(), "Axis")),
            };
            Ok(crate::native::vector_py(axis, vm))
        }

        /// A circle's radius.
        #[pygetset]
        fn Radius(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<f64> {
            match &zelf.curve {
                EdgeCurve::Circle(circle) => Ok(circle.radius()),
                EdgeCurve::Arc(arc) => Ok(arc.circle().radius()),
                _ => Err(no_member(vm, zelf.curve.kind(), "Radius")),
            }
        }

        /// An ellipse's major radius.
        #[pygetset]
        fn MajorRadius(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<f64> {
            match &zelf.curve {
                EdgeCurve::Ellipse(ellipse) => Ok(ellipse.major_radius()),
                EdgeCurve::EllipticalArc(arc) => Ok(arc.ellipse().major_radius()),
                _ => Err(no_member(vm, zelf.curve.kind(), "MajorRadius")),
            }
        }

        /// An ellipse's minor radius.
        #[pygetset]
        fn MinorRadius(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<f64> {
            match &zelf.curve {
                EdgeCurve::Ellipse(ellipse) => Ok(ellipse.minor_radius()),
                EdgeCurve::EllipticalArc(arc) => Ok(arc.ellipse().minor_radius()),
                _ => Err(no_member(vm, zelf.curve.kind(), "MinorRadius")),
            }
        }

        /// The point at `parameter` (`GeometryCurve.value`).
        #[pymethod]
        fn value(zelf: &Py<Self>, parameter: f64, vm: &VirtualMachine) -> PyObjectRef {
            crate::native::vector_py(zelf.curve.point_at(parameter), vm)
        }

        /// The unit tangent at `parameter`.
        #[pymethod]
        fn tangent(zelf: &Py<Self>, parameter: f64, vm: &VirtualMachine) -> PyObjectRef {
            crate::native::vector_py(zelf.curve.tangent_at(parameter), vm)
        }

        /// Whether the curve ends where it starts.
        #[pymethod]
        fn isClosed(zelf: &Py<Self>) -> bool {
            zelf.curve.is_closed()
        }
    }

    impl Representable for PyCurve {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            Ok(format!("<{} object>", zelf.curve.kind()))
        }
    }
}

#[pymodule(name = "Mesh")]
mod mesh {
    #![allow(non_snake_case, non_upper_case_globals)]

    use std::sync::{Arc, Mutex};

    use super::{
        Constructor, FuncArgs, Py, PyObjectRef, PyPayload, PyResult, VirtualMachine, pyclass,
    };

    #[pyattr]
    const __version__: &str = env!("CARGO_PKG_VERSION");

    /// A triangle mesh: `ferrocad_mesh`'s payload over RustPython.
    #[pyattr]
    #[pyclass(name = "Mesh", module = "Mesh")]
    #[derive(Debug, PyPayload, Clone)]
    pub struct PyMesh {
        inner: Arc<Mutex<ferrocad_mesh::Mesh>>,
    }

    impl PyMesh {
        fn of(mesh: ferrocad_mesh::Mesh) -> Self {
            Self {
                inner: Arc::new(Mutex::new(mesh)),
            }
        }

        fn mesh(&self) -> ferrocad_mesh::Mesh {
            self.inner
                .lock()
                .map(|mesh| mesh.clone())
                .unwrap_or_else(|error| error.into_inner().clone())
        }
    }

    pub(super) fn mesh_py(mesh: ferrocad_mesh::Mesh, vm: &VirtualMachine) -> PyObjectRef {
        PyMesh::of(mesh).into_ref(&vm.ctx).into()
    }

    impl Constructor for PyMesh {
        type Args = FuncArgs;

        /// `Mesh()` (empty) or `Mesh(shape)` (meshed at the display deflection).
        fn py_new(
            _cls: &rustpython_vm::Py<rustpython_vm::builtins::PyType>,
            args: Self::Args,
            vm: &VirtualMachine,
        ) -> PyResult<Self> {
            match args.args.len() {
                0 => Ok(Self::of(ferrocad_mesh::Mesh::empty())),
                1 => {
                    let shape = super::shape_of(&args.args[0])
                        .ok_or_else(|| vm.new_type_error("Mesh(shape) expects a Part.Shape"))?;
                    let mesh = super::mesh_from_shape(&shape, 0.1, 0.5)
                        .map_err(|error| vm.new_value_error(error))?;
                    Ok(Self::of(mesh))
                }
                _ => Err(vm.new_type_error("Mesh() takes 0 or 1 arguments")),
            }
        }
    }

    #[pyclass(with(Constructor))]
    impl PyMesh {
        #[pygetset]
        fn Points(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            let points: Vec<PyObjectRef> = zelf
                .mesh()
                .points()
                .iter()
                .map(|point| crate::native::vector_py(*point, vm))
                .collect();
            vm.ctx.new_list(points).into()
        }

        #[pygetset]
        fn Facets(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            let facets: Vec<PyObjectRef> = zelf
                .mesh()
                .facets()
                .iter()
                .map(|[a, b, c]| {
                    vm.ctx
                        .new_tuple(vec![
                            vm.new_pyobj(*a as i64),
                            vm.new_pyobj(*b as i64),
                            vm.new_pyobj(*c as i64),
                        ])
                        .into()
                })
                .collect();
            vm.ctx.new_list(facets).into()
        }

        #[pygetset]
        fn Area(zelf: &Py<Self>) -> f64 {
            zelf.mesh().area()
        }

        #[pygetset]
        fn Volume(zelf: &Py<Self>) -> f64 {
            zelf.mesh().volume()
        }

        #[pygetset]
        fn BoundBox(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            let bounds = zelf.mesh().placed_bounds();
            crate::native::bound_box_py(bounds.min, bounds.max, vm)
        }

        #[pymethod]
        fn countPoints(zelf: &Py<Self>) -> usize {
            zelf.mesh().count_points()
        }

        #[pymethod]
        fn countFacets(zelf: &Py<Self>) -> usize {
            zelf.mesh().count_facets()
        }

        #[pymethod]
        fn isSolid(zelf: &Py<Self>) -> bool {
            zelf.mesh().is_solid()
        }

        #[pymethod]
        fn isValid(zelf: &Py<Self>) -> bool {
            zelf.mesh().is_valid()
        }
    }
}

#[pymodule(name = "MeshPart")]
mod mesh_part {
    #![allow(non_snake_case, non_upper_case_globals)]

    use super::{FuncArgs, PyObjectRef, PyResult, VirtualMachine};

    /// `MeshPart.meshFromShape(Shape=…, LinearDeflection=0.1, AngularDeflection=0.5)`.
    ///
    /// The shape is taken by keyword (upstream's form) or as the first positional
    /// argument; the two deflections are the kernel's tessellation controls.
    #[pyfunction]
    pub fn meshFromShape(args: FuncArgs, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
        let value = args
            .kwargs
            .get("Shape")
            .cloned()
            .or_else(|| args.args.first().cloned())
            .ok_or_else(|| vm.new_type_error("meshFromShape(Shape=…)"))?;
        let shape = super::shape_of(&value)
            .ok_or_else(|| vm.new_type_error("Shape must be a Part.Shape"))?;
        let linear = match args.kwargs.get("LinearDeflection") {
            Some(value) => value.try_float(vm)?.to_f64(),
            None => 0.1,
        };
        let angular = match args.kwargs.get("AngularDeflection") {
            Some(value) => value.try_float(vm)?.to_f64(),
            None => 0.5,
        };
        let mesh = super::mesh_from_shape(&shape, linear, angular)
            .map_err(|error| vm.new_value_error(error))?;
        Ok(super::mesh::mesh_py(mesh, vm))
    }
}
