//! `ferrocad_stage` for RustPython: tools, shapes, snapping, the gizmo and the view.
//!
//! The desktop's `ferrocad_stage` is `ferrocad_stage_py`, a PyO3 extension.
//! `wasm32-unknown-unknown` has no C ABI to load it with, so the browser's interpreter
//! gets a **binding**, not a second implementation: every class here wraps the same
//! `ferrocad_stage` value the CPython module wraps, and every view function publishes
//! into the same process-global `ferrocad_stage::view` a host draws from.
//!
//! The surface is what `python/ferrocad_shell` and the `FreeCADGui` facade actually
//! import:
//!
//! - `Tool` — a subclassable base class; [`PyToolAdapter`] drives an instance through
//!   the native [`fs::Tool`] interface, so `DraftTool` runs in the viewport.
//! - `ShapeItem` and `Stage` — the declarative visuals a tool or preview publishes.
//! - `PointerEvent`/`Hit` and `PointerKind`/`MouseButton`/`DrawMode`.
//! - `Snap`/`SnapKind`/`SnapCandidate`/`SnapContext`, `Workplane`/`WorkplaneKind`.
//! - `Camera`, `TransformGizmo` and its drag/commit events.
//! - The published view (`view_camera`, `set_view_camera`, `preview_items`, …) and the
//!   ranked picker (`pick_at`).
//!
//! Two deliberate differences from the PyO3 binding, both because a RustPython payload
//! is shared behind `&self`:
//!
//! 1. Mutable values (`ShapeItem`, `Stage`, `TransformGizmo`, the tool's item list) store
//!    their native value in a `RefCell`, so a Python setter can write through a getter
//!    descriptor.
//! 2. The CPython module's fieldless `#[pyclass]` enums have no RustPython counterpart
//!    (0.5 cannot derive one), so each variant is a class attribute holding an instance
//!    of the class; `==`/`!=` compare the wrapped native value. The variants are installed
//!    with `#[extend_class]`, because a RustPython `#[pyattr]` runs *while* the type is
//!    being created and an instance of it would meet an uninitialised static type.
//! 3. `Stage` answers `count()` as well as `len()`: a RustPython `#[pymethod]` cannot be
//!    named `__len__`, so the length slot is `AsSequence`'s. Indexing an item out of a
//!    stage is not bound (the PyO3 class does not offer it either).

use ferrocad_core::geometry::{Matrix4, Vector3};
use rustpython_vm::convert::ToPyObject;
use rustpython_vm::function::OptionalArg;
use rustpython_vm::{PyObjectRef, PyResult, VirtualMachine, pymodule};

pub(crate) use stage::extract_tool;
pub(crate) use stage::module_def as stage_module_def;

/// A Python list of any `ToPyObject` values (this RustPython converts only
/// `Vec<PyObjectRef>` directly, so every collection getter comes through here).
fn list_of<T: ToPyObject>(values: impl IntoIterator<Item = T>, vm: &VirtualMachine) -> PyObjectRef {
    let items: Vec<PyObjectRef> = values
        .into_iter()
        .map(|value| value.to_pyobject(vm))
        .collect();
    vm.ctx.new_list(items).into()
}

/// A 4-vector of floats (an RGBA colour).
fn quad_f32(value: &PyObjectRef, vm: &VirtualMachine) -> PyResult<[f32; 4]> {
    let items = items_of(value, vm)?;
    if items.len() != 4 {
        return Err(vm.new_value_error("expected four floats (RGBA)"));
    }
    Ok([
        items[0].try_float(vm)?.to_f64() as f32,
        items[1].try_float(vm)?.to_f64() as f32,
        items[2].try_float(vm)?.to_f64() as f32,
        items[3].try_float(vm)?.to_f64() as f32,
    ])
}

/// The items of a Python list/tuple/iterable, each still a Python object.
fn items_of(value: &PyObjectRef, vm: &VirtualMachine) -> PyResult<Vec<PyObjectRef>> {
    value.clone().try_into_value::<Vec<PyObjectRef>>(vm)
}

/// A `3`-sequence as a native point.
fn point_from_items(items: &[PyObjectRef], vm: &VirtualMachine) -> PyResult<Vector3> {
    if items.len() != 3 {
        return Err(vm.new_value_error("a point needs exactly three coordinates"));
    }
    Ok(Vector3::new(
        items[0].try_float(vm)?.to_f64(),
        items[1].try_float(vm)?.to_f64(),
        items[2].try_float(vm)?.to_f64(),
    ))
}

/// A `3`-tuple, a `3`-list or a `FreeCAD.Vector`, as a native point.
fn point_of(value: &PyObjectRef, vm: &VirtualMachine) -> PyResult<Vector3> {
    if let Some(vector) = crate::native::vector_of(value) {
        return Ok(vector);
    }
    match items_of(value, vm) {
        Ok(items) => point_from_items(&items, vm),
        Err(_) => Err(vm.new_type_error("expected an (x, y, z) tuple or a FreeCAD.Vector")),
    }
}

/// A `2`-tuple of device-pixel coordinates.
fn pair_i32(value: &PyObjectRef, vm: &VirtualMachine) -> PyResult<(i32, i32)> {
    let items = items_of(value, vm)?;
    if items.len() != 2 {
        return Err(vm.new_value_error("expected a two-item (x, y) position"));
    }
    Ok((
        items[0].clone().try_into_value::<i32>(vm)?,
        items[1].clone().try_into_value::<i32>(vm)?,
    ))
}

/// A `2`-tuple of non-negative integers (a viewport size).
fn pair_u32(value: &PyObjectRef, vm: &VirtualMachine) -> PyResult<(u32, u32)> {
    let items = items_of(value, vm)?;
    if items.len() != 2 {
        return Err(vm.new_value_error("expected a two-item (width, height) size"));
    }
    Ok((
        items[0].clone().try_into_value::<u32>(vm)?,
        items[1].clone().try_into_value::<u32>(vm)?,
    ))
}

/// `N` 3-vectors: a sequence of triples, or a flat sequence of floats.
fn vec3_list(value: &PyObjectRef, what: &str, vm: &VirtualMachine) -> PyResult<Vec<[f32; 3]>> {
    let items = items_of(value, vm)?;
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let mut triples = Vec::with_capacity(items.len());
    let mut all_triples = true;
    for item in &items {
        match point_of(item, vm) {
            Ok(point) => triples.push([point.x as f32, point.y as f32, point.z as f32]),
            Err(_) => {
                all_triples = false;
                break;
            }
        }
    }
    if all_triples {
        return Ok(triples);
    }
    if items.len() % 3 != 0 {
        return Err(vm.new_value_error(format!(
            "{what} needs a multiple of 3 floats, got {}",
            items.len()
        )));
    }
    let mut flat = Vec::with_capacity(items.len());
    for item in &items {
        flat.push(item.try_float(vm)?.to_f64() as f32);
    }
    Ok(flat.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect())
}

/// A flat index array of non-negative integers.
fn u32_list(value: &PyObjectRef, _what: &str, vm: &VirtualMachine) -> PyResult<Vec<u32>> {
    items_of(value, vm)?
        .iter()
        .map(|item| item.clone().try_into_value::<u32>(vm))
        .collect()
}

/// The optional form of [`vec3_list`].
fn opt_vec3(
    value: OptionalArg<PyObjectRef>,
    what: &str,
    vm: &VirtualMachine,
) -> PyResult<Vec<[f32; 3]>> {
    match value.into_option() {
        Some(value) if !vm.is_none(&value) => vec3_list(&value, what, vm),
        _ => Ok(Vec::new()),
    }
}

/// The optional form of [`u32_list`].
fn opt_u32(value: OptionalArg<PyObjectRef>, what: &str, vm: &VirtualMachine) -> PyResult<Vec<u32>> {
    match value.into_option() {
        Some(value) if !vm.is_none(&value) => u32_list(&value, what, vm),
        _ => Ok(Vec::new()),
    }
}

/// A Python number as `f64`. `int` is accepted, which `f64::try_from_object` is not.
fn number(value: &PyObjectRef, what: &str, vm: &VirtualMachine) -> PyResult<f64> {
    value
        .try_float(vm)
        .map(|value| value.to_f64())
        .map_err(|_| vm.new_type_error(format!("{what} must be a number")))
}

fn triple(v: [f32; 3]) -> (f32, f32, f32) {
    (v[0], v[1], v[2])
}

fn triple_f64((x, y, z): (f64, f64, f64)) -> Vector3 {
    Vector3::new(x, y, z)
}

fn as_tuple(v: Vector3) -> (f64, f64, f64) {
    (v.x, v.y, v.z)
}

/// A row-major 4×4 matrix from 16 floats.
fn matrix(values: &[f64], what: &str, vm: &VirtualMachine) -> PyResult<Matrix4> {
    let values: [f64; 16] = values.try_into().map_err(|_| {
        vm.new_value_error(format!("{what} must be 16 floats, got {}", values.len()))
    })?;
    Ok(Matrix4::from_values(values))
}

/// `ferrocad_stage` for RustPython.
#[pymodule(name = "ferrocad_stage")]
mod stage {
    #![allow(non_snake_case, non_upper_case_globals)]

    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    use ferrocad_core::geometry::{Placement, Rotation, Vector3};
    use ferrocad_graphics as gfx;
    use ferrocad_stage as fs;
    use rustpython_vm::builtins::{PyDict, PyType, PyTypeRef};
    use rustpython_vm::function::{FuncArgs, OptionalArg, PyComparisonValue};
    use rustpython_vm::protocol::PySequenceMethods;
    use rustpython_vm::types::{
        AsSequence, Comparable, Constructor, PyComparisonOp, Representable,
    };
    use rustpython_vm::{
        Context, Interpreter, Py, PyObject, PyObjectRef, PyPayload, PyRef, PyResult,
        VirtualMachine, pyclass,
    };

    use super::{
        as_tuple, list_of, matrix, number, opt_u32, opt_vec3, pair_i32, pair_u32, point_of,
        quad_f32, triple, triple_f64, vec3_list,
    };

    #[pyattr]
    const __version__: &str = env!("CARGO_PKG_VERSION");

    /// Compare two wrapper types by their native value.
    macro_rules! compare_by_value {
        ($name:ty) => {
            impl Comparable for $name {
                fn cmp(
                    zelf: &Py<Self>,
                    other: &PyObject,
                    op: PyComparisonOp,
                    _vm: &VirtualMachine,
                ) -> PyResult<PyComparisonValue> {
                    op.eq_only(|| match other.downcast_ref::<Self>() {
                        Some(other) => Ok((zelf.inner == other.inner).into()),
                        None => Ok(PyComparisonValue::NotImplemented),
                    })
                }
            }
        };
    }

    /// Publish each variant as a class attribute holding an instance of the class.
    ///
    /// RustPython cannot derive a fieldless enum, and `#[pyattr]` runs *while* the
    /// type is being created, so an instance built there would meet an uninitialised
    /// static type. `#[extend_class]` runs afterwards (with the type in hand), which is
    /// why the variants are installed here.
    macro_rules! enum_variants {
        ($name:ident, $ctx:ident, $class:ident, $(($py:literal, $variant:expr)),* $(,)?) => {
            $(
                {
                    let object: PyObjectRef =
                        PyRef::new_ref($name { inner: $variant }, $class.to_owned(), None)
                            .into();
                    $class.set_str_attr($py, object, $ctx);
                }
            )*
        };
    }

    // -----------------------------------------------------------------------
    // Enums
    // -----------------------------------------------------------------------

    /// `ferrocad_stage::PointerKind`.
    #[pyattr]
    #[pyclass(name = "PointerKind", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone, Copy)]
    pub struct PyPointerKind {
        inner: fs::PointerKind,
    }

    #[pyclass(with(Comparable))]
    impl PyPointerKind {
        #[extend_class]
        fn extend_class(ctx: &Context, class: &'static Py<PyType>) {
            enum_variants!(
                PyPointerKind,
                ctx,
                class,
                ("Move", fs::PointerKind::Move),
                ("Down", fs::PointerKind::Down),
                ("Up", fs::PointerKind::Up),
            );
        }
    }

    compare_by_value!(PyPointerKind);

    /// `ferrocad_stage::MouseButton`.
    #[pyattr]
    #[pyclass(name = "MouseButton", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone, Copy)]
    pub struct PyMouseButton {
        inner: fs::MouseButton,
    }

    #[pyclass(with(Comparable))]
    impl PyMouseButton {
        #[extend_class]
        fn extend_class(ctx: &Context, class: &'static Py<PyType>) {
            enum_variants!(
                PyMouseButton,
                ctx,
                class,
                ("Left", fs::MouseButton::Left),
                ("Middle", fs::MouseButton::Middle),
                ("Right", fs::MouseButton::Right),
            );
        }
    }

    compare_by_value!(PyMouseButton);

    /// `ferrocad_stage::DrawMode`.
    #[pyattr]
    #[pyclass(name = "DrawMode", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone, Copy)]
    pub struct PyDrawMode {
        inner: fs::DrawMode,
    }

    #[pyclass(with(Comparable))]
    impl PyDrawMode {
        #[extend_class]
        fn extend_class(ctx: &Context, class: &'static Py<PyType>) {
            enum_variants!(
                PyDrawMode,
                ctx,
                class,
                ("Shaded", fs::DrawMode::Shaded),
                ("Wireframe", fs::DrawMode::Wireframe),
                ("ShadedWithEdges", fs::DrawMode::ShadedWithEdges),
            );
        }
    }

    compare_by_value!(PyDrawMode);

    /// `ferrocad_stage::SnapKind`.
    #[pyattr]
    #[pyclass(name = "SnapKind", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone, Copy)]
    pub struct PySnapKind {
        inner: fs::SnapKind,
    }

    #[pyclass(with(Comparable))]
    impl PySnapKind {
        #[extend_class]
        fn extend_class(ctx: &Context, class: &'static Py<PyType>) {
            enum_variants!(
                PySnapKind,
                ctx,
                class,
                ("Point", fs::SnapKind::Point),
                ("Grid", fs::SnapKind::Grid),
                ("Vertex", fs::SnapKind::Vertex),
                ("EdgeMid", fs::SnapKind::EdgeMid),
                ("Centre", fs::SnapKind::Centre),
                ("Origin", fs::SnapKind::Origin),
            );
        }
    }

    compare_by_value!(PySnapKind);

    /// `ferrocad_stage::GizmoMode`.
    #[pyattr]
    #[pyclass(name = "GizmoMode", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone, Copy)]
    pub struct PyGizmoMode {
        inner: fs::GizmoMode,
    }

    #[pyclass(with(Comparable))]
    impl PyGizmoMode {
        #[extend_class]
        fn extend_class(ctx: &Context, class: &'static Py<PyType>) {
            enum_variants!(
                PyGizmoMode,
                ctx,
                class,
                ("Translate", fs::GizmoMode::Translate),
                ("Rotate", fs::GizmoMode::Rotate),
                ("Universal", fs::GizmoMode::Universal),
            );
        }
    }

    compare_by_value!(PyGizmoMode);

    /// `ferrocad_stage::GizmoHandle`.
    #[pyattr]
    #[pyclass(name = "GizmoHandle", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone, Copy)]
    pub struct PyGizmoHandle {
        inner: fs::GizmoHandle,
    }

    #[pyclass(with(Comparable))]
    impl PyGizmoHandle {
        #[extend_class]
        fn extend_class(ctx: &Context, class: &'static Py<PyType>) {
            enum_variants!(
                PyGizmoHandle,
                ctx,
                class,
                ("AxisX", fs::GizmoHandle::AxisX),
                ("AxisY", fs::GizmoHandle::AxisY),
                ("AxisZ", fs::GizmoHandle::AxisZ),
                ("PlaneXY", fs::GizmoHandle::PlaneXY),
                ("PlaneXZ", fs::GizmoHandle::PlaneXZ),
                ("PlaneYZ", fs::GizmoHandle::PlaneYZ),
                ("RotateX", fs::GizmoHandle::RotateX),
                ("RotateY", fs::GizmoHandle::RotateY),
                ("RotateZ", fs::GizmoHandle::RotateZ),
            );
        }
    }

    compare_by_value!(PyGizmoHandle);

    // -----------------------------------------------------------------------
    // Picking and pointer input
    // -----------------------------------------------------------------------

    /// A pick result (`ferrocad_graphics::Hit`).
    #[pyattr]
    #[pyclass(name = "Hit", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone, Copy)]
    pub struct PyHit {
        inner: gfx::Hit,
    }

    #[pyclass(with(Representable))]
    impl PyHit {
        /// The object that was hit (the packet's pick id).
        #[pygetset]
        fn entity_id(zelf: &Py<Self>) -> u32 {
            zelf.inner.id.0
        }

        /// The topological sub-element id (`Vertex.part_idx`); `0` when untagged.
        #[pygetset]
        fn part_idx(zelf: &Py<Self>) -> u32 {
            zelf.inner.part
        }

        /// The primitive index within the mesh, when the pick path knows it.
        #[pygetset]
        fn element(zelf: &Py<Self>) -> Option<u32> {
            zelf.inner.element
        }

        /// The world-space intersection point.
        #[pygetset]
        fn point(zelf: &Py<Self>) -> (f32, f32, f32) {
            triple(zelf.inner.point)
        }

        /// The world-space surface normal.
        #[pygetset]
        fn normal(zelf: &Py<Self>) -> (f32, f32, f32) {
            triple(zelf.inner.normal)
        }

        /// The distance along the ray from its origin.
        #[pygetset]
        fn distance(zelf: &Py<Self>) -> f64 {
            zelf.inner.distance
        }

        /// The topology of the hit: `"Triangles"`, `"Lines"` or `"Points"`.
        #[pygetset]
        fn topology(zelf: &Py<Self>) -> &'static str {
            match zelf.inner.topology {
                gfx::Topology::Triangles => "Triangles",
                gfx::Topology::Lines => "Lines",
                gfx::Topology::Points => "Points",
            }
        }
    }

    impl Representable for PyHit {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            Ok(format!(
                "Hit(entity_id={}, part_idx={}, point={:?})",
                zelf.inner.id.0,
                zelf.inner.part,
                triple(zelf.inner.point)
            ))
        }
    }

    /// A structured pointer event (device pixels, bottom-left origin).
    #[pyattr]
    #[pyclass(name = "PointerEvent", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone, Copy)]
    pub struct PyPointerEvent {
        inner: fs::PointerEvent,
        /// The resolved snap point, when the host supplied one.
        snap: Option<[f32; 3]>,
    }

    impl PyPointerEvent {
        fn build(
            kind: fs::PointerKind,
            position: (i32, i32),
            button: Option<fs::MouseButton>,
            hit: Option<gfx::Hit>,
        ) -> Self {
            Self {
                inner: fs::PointerEvent {
                    kind,
                    position: [position.0, position.1],
                    button,
                    hit,
                    projection: None,
                },
                snap: None,
            }
        }
    }

    impl Constructor for PyPointerEvent {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, args: Self::Args, vm: &VirtualMachine) -> PyResult<Self> {
            let kind = args
                .args
                .first()
                .and_then(|value| value.downcast_ref::<PyPointerKind>())
                .map(|kind| kind.inner)
                .ok_or_else(|| {
                    vm.new_type_error("PointerEvent(kind, position, button=None, hit=None)")
                })?;
            let position = args
                .args
                .get(1)
                .map(|value| pair_i32(value, vm))
                .transpose()?
                .ok_or_else(|| vm.new_type_error("PointerEvent needs a position"))?;
            let button = args
                .args
                .get(2)
                .and_then(|value| value.downcast_ref::<PyMouseButton>())
                .map(|button| button.inner);
            let hit = args
                .args
                .get(3)
                .and_then(|value| value.downcast_ref::<PyHit>())
                .map(|hit| hit.inner);
            Ok(Self::build(kind, position, button, hit))
        }
    }

    #[pyclass(with(Constructor, Representable))]
    impl PyPointerEvent {
        /// A pointer-move event at `position`.
        #[pystaticmethod(name = "move")]
        fn move_at(position: (i32, i32)) -> Self {
            Self::build(fs::PointerKind::Move, position, None, None)
        }

        /// A button-press event at `position`.
        #[pystaticmethod]
        fn down(position: (i32, i32), button: PyRef<PyMouseButton>) -> Self {
            Self::build(fs::PointerKind::Down, position, Some(button.inner), None)
        }

        /// A button-release event at `position`.
        #[pystaticmethod]
        fn up(position: (i32, i32), button: PyRef<PyMouseButton>) -> Self {
            Self::build(fs::PointerKind::Up, position, Some(button.inner), None)
        }

        /// Which event this is.
        #[pygetset]
        fn kind(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            PyPointerKind {
                inner: zelf.inner.kind,
            }
            .into_ref(&vm.ctx)
            .into()
        }

        /// The device-pixel position (bottom-left origin).
        #[pygetset]
        fn position(zelf: &Py<Self>) -> (i32, i32) {
            (zelf.inner.position[0], zelf.inner.position[1])
        }

        /// The button, for a `Down`/`Up` event.
        #[pygetset]
        fn button(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            match zelf.inner.button {
                Some(button) => PyMouseButton { inner: button }.into_ref(&vm.ctx).into(),
                None => vm.ctx.none(),
            }
        }

        /// The pick under the pointer, if it hit geometry.
        #[pygetset]
        fn hit(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            match zelf.inner.hit {
                Some(hit) => PyHit { inner: hit }.into_ref(&vm.ctx).into(),
                None => vm.ctx.none(),
            }
        }

        /// The resolved snap point, if the host supplied one.
        #[pygetset]
        fn snap(zelf: &Py<Self>) -> Option<(f32, f32, f32)> {
            zelf.snap.map(triple)
        }

        /// The world-space point under the pointer, if it hit geometry.
        #[pymethod]
        fn world(zelf: &Py<Self>) -> Option<(f32, f32, f32)> {
            zelf.inner.world().map(triple)
        }

        /// Return a copy carrying `snap` (set by a host before dispatch).
        #[pymethod]
        fn with_snap(
            zelf: &Py<Self>,
            snap: OptionalArg<PyObjectRef>,
            vm: &VirtualMachine,
        ) -> PyResult<Self> {
            let snap = match snap.into_option().filter(|value| !vm.is_none(value)) {
                Some(value) => {
                    let point = point_of(&value, vm)?;
                    Some([point.x as f32, point.y as f32, point.z as f32])
                }
                None => None,
            };
            Ok(Self {
                inner: zelf.inner,
                snap,
            })
        }
    }

    impl Representable for PyPointerEvent {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            Ok(format!(
                "PointerEvent(kind={:?}, position={:?}, button={:?})",
                zelf.inner.kind, zelf.inner.position, zelf.inner.button
            ))
        }
    }

    // -----------------------------------------------------------------------
    // Shapes and stages
    // -----------------------------------------------------------------------

    /// A declarative visual item (`ferrocad_stage::ShapeItem`).
    #[pyattr]
    #[pyclass(name = "ShapeItem", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload)]
    pub struct PyShapeItem {
        item: RefCell<fs::ShapeItem>,
    }

    impl PyShapeItem {
        fn of(item: fs::ShapeItem) -> Self {
            Self {
                item: RefCell::new(item),
            }
        }
    }

    impl Constructor for PyShapeItem {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, args: Self::Args, vm: &VirtualMachine) -> PyResult<Self> {
            let entity_id = args
                .args
                .first()
                .map(|value| value.clone().try_into_value::<u32>(vm))
                .transpose()?
                .unwrap_or(0);
            Ok(Self::of(fs::ShapeItem::new(entity_id)))
        }
    }

    #[pyclass(with(Constructor, Representable))]
    impl PyShapeItem {
        /// Replace the tessellation.
        #[pymethod]
        fn set_shape(
            zelf: &Py<Self>,
            positions: PyObjectRef,
            normals: OptionalArg<PyObjectRef>,
            triangles: OptionalArg<PyObjectRef>,
            edges: OptionalArg<PyObjectRef>,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            let positions = vec3_list(&positions, "positions", vm)?;
            let normals = opt_vec3(normals, "normals", vm)?;
            let triangles = opt_u32(triangles, "triangles", vm)?;
            let edges = opt_u32(edges, "edges", vm)?;
            zelf.item
                .borrow_mut()
                .set_shape(positions, normals, triangles, edges);
            Ok(())
        }

        /// Replace the tessellation and tag each position with its sub-element id.
        #[pymethod]
        fn set_mesh(
            zelf: &Py<Self>,
            positions: PyObjectRef,
            normals: OptionalArg<PyObjectRef>,
            triangles: OptionalArg<PyObjectRef>,
            edges: OptionalArg<PyObjectRef>,
            parts: OptionalArg<PyObjectRef>,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            let positions = vec3_list(&positions, "positions", vm)?;
            let normals = opt_vec3(normals, "normals", vm)?;
            let triangles = opt_u32(triangles, "triangles", vm)?;
            let edges = opt_u32(edges, "edges", vm)?;
            let parts = opt_u32(parts, "parts", vm)?;
            zelf.item
                .borrow_mut()
                .set_mesh(positions, normals, triangles, edges, parts);
            Ok(())
        }

        /// The pick id.
        #[pygetset]
        fn entity_id(zelf: &Py<Self>) -> u32 {
            zelf.item.borrow().entity_id()
        }

        /// The sub-element name table (`part id → "Face6"`).
        #[pygetset]
        fn part_names(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            list_of(zelf.item.borrow().part_names().iter().cloned(), vm)
        }

        /// The per-position sub-element ids (empty when untagged).
        #[pygetset]
        fn parts(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            list_of(zelf.item.borrow().parts().iter().copied(), vm)
        }

        /// The display mode.
        #[pygetset]
        fn draw_mode(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            PyDrawMode {
                inner: zelf.item.borrow().draw_mode(),
            }
            .into_ref(&vm.ctx)
            .into()
        }

        #[pygetset(setter)]
        fn set_draw_mode(zelf: &Py<Self>, mode: PyRef<PyDrawMode>) {
            let item = zelf.item.borrow().clone().with_draw_mode(mode.inner);
            *zelf.item.borrow_mut() = item;
        }

        /// The surface colour (RGBA).
        #[pygetset]
        fn surface_color(zelf: &Py<Self>) -> (f32, f32, f32, f32) {
            let color = zelf.item.borrow().surface_color();
            (color[0], color[1], color[2], color[3])
        }

        #[pygetset(setter)]
        fn set_surface_color(
            zelf: &Py<Self>,
            color: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            zelf.item
                .borrow_mut()
                .set_surface_color(quad_f32(&color, vm)?);
            Ok(())
        }

        /// The edge colour (RGBA).
        #[pygetset]
        fn edge_color(zelf: &Py<Self>) -> (f32, f32, f32, f32) {
            let color = zelf.item.borrow().edge_color();
            (color[0], color[1], color[2], color[3])
        }

        #[pygetset(setter)]
        fn set_edge_color(
            zelf: &Py<Self>,
            color: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            zelf.item.borrow_mut().set_edge_color(quad_f32(&color, vm)?);
            Ok(())
        }

        /// The edge width (stored, not yet consumed by the renderer).
        #[pygetset]
        fn edge_width(zelf: &Py<Self>) -> f32 {
            zelf.item.borrow().edge_width()
        }

        #[pygetset(setter)]
        fn set_edge_width(zelf: &Py<Self>, width: f32) {
            let item = zelf.item.borrow().clone().with_edge_width(width);
            *zelf.item.borrow_mut() = item;
        }

        /// Whether the shape participates in picking.
        #[pygetset]
        fn selectable(zelf: &Py<Self>) -> bool {
            zelf.item.borrow().selectable()
        }

        #[pygetset(setter)]
        fn set_selectable(zelf: &Py<Self>, selectable: bool) {
            let item = zelf.item.borrow().clone().with_selectable(selectable);
            *zelf.item.borrow_mut() = item;
        }

        /// The temporary preview transform (16 row-major floats), if any.
        #[pygetset]
        fn temporary_transform(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            match zelf.item.borrow().temporary_transform() {
                Some(matrix) => list_of(matrix.values().iter().copied(), vm),
                None => vm.ctx.none(),
            }
        }

        #[pygetset(setter)]
        fn set_temporary_transform(
            zelf: &Py<Self>,
            transform: Option<Vec<f64>>,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            let transform = transform
                .map(|values| matrix(&values, "temporary_transform", vm))
                .transpose()?;
            zelf.item.borrow_mut().set_temporary_transform(transform);
            Ok(())
        }

        /// The vertex positions (copies).
        #[pygetset]
        fn positions(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            list_of(
                zelf.item.borrow().positions().iter().map(|p| triple(*p)),
                vm,
            )
        }

        /// The per-vertex normals (empty when flat-shaded).
        #[pygetset]
        fn normals(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            list_of(zelf.item.borrow().normals().iter().map(|p| triple(*p)), vm)
        }

        /// The flat triangle indices, three per triangle.
        #[pygetset]
        fn triangles(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            list_of(zelf.item.borrow().triangles().iter().copied(), vm)
        }

        /// The flat edge segment indices, two per segment.
        #[pygetset]
        fn edges(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            list_of(zelf.item.borrow().edges().iter().copied(), vm)
        }
    }

    impl Representable for PyShapeItem {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            let item = zelf.item.borrow();
            Ok(format!(
                "ShapeItem(entity_id={}, draw_mode={:?})",
                item.entity_id(),
                item.draw_mode()
            ))
        }
    }

    /// A stage of items (`ferrocad_stage::Stage`).
    #[pyattr]
    #[pyclass(name = "Stage", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload)]
    pub struct PyStage {
        inner: RefCell<fs::Stage>,
    }

    impl Constructor for PyStage {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, _args: Self::Args, _vm: &VirtualMachine) -> PyResult<Self> {
            Ok(Self {
                inner: RefCell::new(fs::Stage::new()),
            })
        }
    }

    #[pyclass(with(Constructor, Representable, AsSequence))]
    impl PyStage {
        /// Append a copy of `item` (drawn last).
        #[pymethod]
        fn add(zelf: &Py<Self>, item: PyRef<PyShapeItem>) {
            zelf.inner.borrow_mut().add_item(item.item.borrow().clone());
        }

        /// Drop every item.
        #[pymethod]
        fn clear(zelf: &Py<Self>) {
            *zelf.inner.borrow_mut() = fs::Stage::new();
        }

        /// The number of items.
        #[pymethod]
        fn count(zelf: &Py<Self>) -> usize {
            zelf.inner.borrow().len()
        }

        /// Whether the stage has no items.
        #[pymethod]
        fn is_empty(zelf: &Py<Self>) -> bool {
            zelf.inner.borrow().is_empty()
        }

        /// How many draw packets the stage compiles to right now.
        #[pymethod]
        fn batch_count(zelf: &Py<Self>) -> usize {
            zelf.inner.borrow().to_queue().len()
        }

        /// The pick ids of the items, in draw order.
        #[pymethod]
        fn entities(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            list_of(
                zelf.inner
                    .borrow()
                    .items()
                    .iter()
                    .map(|item| item.entity_id()),
                vm,
            )
        }
    }

    impl AsSequence for PyStage {
        fn as_sequence() -> &'static PySequenceMethods {
            static METHODS: PySequenceMethods = PySequenceMethods {
                length: Some(|seq, _vm| Ok(PyStage::sequence_downcast(seq).inner.borrow().len())),
                ..PySequenceMethods::NOT_IMPLEMENTED
            };
            &METHODS
        }
    }

    impl Representable for PyStage {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            Ok(format!("Stage(items={})", zelf.inner.borrow().len()))
        }
    }

    // -----------------------------------------------------------------------
    // Snapping and workplanes
    // -----------------------------------------------------------------------

    /// A resolved snap (`ferrocad_stage::Snap`).
    #[pyattr]
    #[pyclass(name = "Snap", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone, Copy)]
    pub struct PySnap {
        inner: fs::Snap,
    }

    #[pyclass(with(Representable))]
    impl PySnap {
        /// The snapped world-space point (`(x, y, z)`).
        #[pygetset]
        fn point(zelf: &Py<Self>) -> (f64, f64, f64) {
            (
                f64::from(zelf.inner.point[0]),
                f64::from(zelf.inner.point[1]),
                f64::from(zelf.inner.point[2]),
            )
        }

        /// What it snapped to.
        #[pygetset]
        fn kind(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            PySnapKind {
                inner: zelf.inner.kind,
            }
            .into_ref(&vm.ctx)
            .into()
        }

        /// The local `(u, v)` on the active workplane, when one is known.
        #[pygetset]
        fn local(zelf: &Py<Self>) -> Option<(f64, f64)> {
            zelf.inner.local.map(|local| (local[0], local[1]))
        }

        /// Whether a grid snap landed on a **major** intersection.
        #[pygetset]
        fn major(zelf: &Py<Self>) -> bool {
            zelf.inner.major
        }
    }

    impl Representable for PySnap {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            let point = (
                f64::from(zelf.inner.point[0]),
                f64::from(zelf.inner.point[1]),
                f64::from(zelf.inner.point[2]),
            );
            Ok(format!(
                "Snap(point={point:?}, kind={:?}, local={:?}, major={})",
                zelf.inner.kind, zelf.inner.local, zelf.inner.major
            ))
        }
    }

    /// A world-space point a snap may resolve to (`ferrocad_stage::SnapCandidate`).
    #[pyattr]
    #[pyclass(name = "SnapCandidate", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone, Copy)]
    pub struct PySnapCandidate {
        inner: fs::SnapCandidate,
    }

    impl Constructor for PySnapCandidate {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, args: Self::Args, vm: &VirtualMachine) -> PyResult<Self> {
            let point = args
                .args
                .first()
                .map(|value| point_of(value, vm))
                .transpose()?
                .ok_or_else(|| vm.new_type_error("SnapCandidate(point, kind)"))?;
            let kind = args
                .args
                .get(1)
                .and_then(|value| value.downcast_ref::<PySnapKind>())
                .map(|kind| kind.inner)
                .ok_or_else(|| vm.new_type_error("SnapCandidate(point, kind)"))?;
            Ok(Self {
                inner: fs::SnapCandidate { point, kind },
            })
        }
    }

    #[pyclass(with(Constructor, Representable))]
    impl PySnapCandidate {
        /// The candidate's world-space point.
        #[pygetset]
        fn point(zelf: &Py<Self>) -> (f64, f64, f64) {
            as_tuple(zelf.inner.point)
        }

        /// The kind of feature this candidate is.
        #[pygetset]
        fn kind(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            PySnapKind {
                inner: zelf.inner.kind,
            }
            .into_ref(&vm.ctx)
            .into()
        }
    }

    impl Representable for PySnapCandidate {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            Ok(format!(
                "SnapCandidate(point={:?}, kind={:?})",
                as_tuple(zelf.inner.point),
                zelf.inner.kind
            ))
        }
    }

    /// The plane and steps a snap is resolved against (`ferrocad_stage::SnapContext`).
    #[pyattr]
    #[pyclass(name = "SnapContext", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone)]
    pub struct PySnapContext {
        inner: fs::SnapContext,
    }

    impl Constructor for PySnapContext {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, args: Self::Args, vm: &VirtualMachine) -> PyResult<Self> {
            let workplane = args
                .args
                .first()
                .and_then(|value| value.downcast_ref::<PyWorkplane>())
                .map(|workplane| workplane.inner.clone())
                .ok_or_else(|| vm.new_type_error("SnapContext(workplane, minor, major)"))?;
            let minor = args
                .args
                .get(1)
                .map(|value| number(value, "minor", vm))
                .transpose()?
                .unwrap_or(0.0);
            let major = args
                .args
                .get(2)
                .map(|value| number(value, "major", vm))
                .transpose()?
                .unwrap_or(minor * 10.0);
            Ok(Self {
                inner: fs::SnapContext {
                    workplane,
                    minor,
                    major,
                },
            })
        }
    }

    #[pyclass(with(Constructor, Representable))]
    impl PySnapContext {
        /// The world `XY` plane with `minor` and a decade-major step.
        #[pyclassmethod]
        fn world(_cls: PyTypeRef, minor: f64) -> Self {
            Self {
                inner: fs::SnapContext::world(minor),
            }
        }

        /// The workplane the grid lives on.
        #[pygetset]
        fn workplane(zelf: &Py<Self>) -> PyWorkplane {
            PyWorkplane {
                inner: zelf.inner.workplane.clone(),
            }
        }

        /// The minor (finest) grid step.
        #[pygetset]
        fn minor(zelf: &Py<Self>) -> f64 {
            zelf.inner.minor
        }

        /// The major step.
        #[pygetset]
        fn major(zelf: &Py<Self>) -> f64 {
            zelf.inner.major
        }

        /// Resolve a snap for a world-space `point`.
        #[pymethod]
        fn snap_point(zelf: &Py<Self>, args: FuncArgs, vm: &VirtualMachine) -> PyResult<PySnap> {
            let point = args
                .args
                .first()
                .map(|value| point_of(value, vm))
                .transpose()?
                .ok_or_else(|| {
                    vm.new_type_error("snap_point(point, candidates=(), tolerance=0.0)")
                })?;
            let mut candidates = Vec::new();
            if let Some(value) = args.args.get(1).filter(|value| !vm.is_none(value)) {
                for item in value.clone().try_into_value::<Vec<PyObjectRef>>(vm)? {
                    if let Some(candidate) = item.downcast_ref::<PySnapCandidate>() {
                        candidates.push(candidate.inner);
                    }
                }
            }
            let tolerance = match args.args.get(2) {
                Some(value) if !vm.is_none(value) => number(value, "tolerance", vm)?,
                _ => 0.0,
            };
            Ok(PySnap {
                inner: zelf.inner.snap_point(point, &candidates, tolerance),
            })
        }
    }

    impl Representable for PySnapContext {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            Ok(format!(
                "SnapContext(workplane={:?}, minor={}, major={})",
                zelf.inner.workplane, zelf.inner.minor, zelf.inner.major
            ))
        }
    }

    /// Where a `Workplane` came from (`ferrocad_stage::WorkplaneKind`).
    #[pyattr]
    #[pyclass(name = "WorkplaneKind", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone)]
    pub struct PyWorkplaneKind {
        inner: fs::WorkplaneKind,
    }

    #[pyclass(with(Comparable, Representable))]
    impl PyWorkplaneKind {
        #[extend_class]
        fn extend_class(ctx: &Context, class: &'static Py<PyType>) {
            enum_variants!(
                PyWorkplaneKind,
                ctx,
                class,
                ("World", fs::WorkplaneKind::World),
                ("Custom", fs::WorkplaneKind::Custom),
            );
        }

        /// A named datum plane.
        #[pystaticmethod(name = "Datum")]
        fn datum(name: String) -> Self {
            Self {
                inner: fs::WorkplaneKind::Datum(name),
            }
        }

        /// A planar face of the object `entity`, the sub-element `element`.
        #[pystaticmethod(name = "Face")]
        fn face(entity: u32, element: String) -> Self {
            Self {
                inner: fs::WorkplaneKind::Face { entity, element },
            }
        }

        /// The readable value.
        #[pygetset]
        fn kind(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            match &zelf.inner {
                fs::WorkplaneKind::World => vm.ctx.new_str("World").into(),
                fs::WorkplaneKind::Datum(name) => vm
                    .ctx
                    .new_tuple(vec![
                        vm.ctx.new_str("Datum").into(),
                        vm.new_pyobj(name.clone()),
                    ])
                    .into(),
                fs::WorkplaneKind::Face { entity, element } => vm
                    .ctx
                    .new_tuple(vec![
                        vm.ctx.new_str("Face").into(),
                        vm.new_pyobj(*entity),
                        vm.new_pyobj(element.clone()),
                    ])
                    .into(),
                fs::WorkplaneKind::Custom => vm.ctx.new_str("Custom").into(),
            }
        }
    }

    impl Representable for PyWorkplaneKind {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            Ok(match &zelf.inner {
                fs::WorkplaneKind::World => "WorkplaneKind.World".to_string(),
                fs::WorkplaneKind::Datum(name) => format!("WorkplaneKind.Datum({name:?})"),
                fs::WorkplaneKind::Face { entity, element } => {
                    format!("WorkplaneKind.Face({entity}, {element:?})")
                }
                fs::WorkplaneKind::Custom => "WorkplaneKind.Custom".to_string(),
            })
        }
    }

    compare_by_value!(PyWorkplaneKind);

    /// A right-handed planar frame (`ferrocad_stage::Workplane`).
    #[pyattr]
    #[pyclass(name = "Workplane", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone)]
    pub struct PyWorkplane {
        inner: fs::Workplane,
    }

    impl Constructor for PyWorkplane {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, args: Self::Args, vm: &VirtualMachine) -> PyResult<Self> {
            let origin = match args.args.first() {
                Some(value) => point_of(value, vm)?,
                None => Vector3::zero(),
            };
            let u = match args.args.get(1) {
                Some(value) => point_of(value, vm)?,
                None => Vector3::new(1.0, 0.0, 0.0),
            };
            let v = match args.args.get(2) {
                Some(value) => point_of(value, vm)?,
                None => Vector3::new(0.0, 1.0, 0.0),
            };
            let kind = args
                .args
                .get(3)
                .and_then(|value| value.downcast_ref::<PyWorkplaneKind>())
                .map(|kind| kind.inner.clone())
                .unwrap_or(fs::WorkplaneKind::World);
            fs::Workplane::new(origin, u, v, kind)
                .map(|inner| Self { inner })
                .ok_or_else(|| {
                    vm.new_value_error("the workplane axes must be non-degenerate and not parallel")
                })
        }
    }

    #[pyclass(with(Constructor, Representable))]
    impl PyWorkplane {
        /// The world `XY` plane.
        #[pyclassmethod]
        fn world(_cls: PyTypeRef) -> Self {
            Self {
                inner: fs::Workplane::world(),
            }
        }

        /// The origin.
        #[pygetset]
        fn origin(zelf: &Py<Self>) -> (f64, f64, f64) {
            as_tuple(zelf.inner.origin)
        }

        /// The `u` axis (normalized).
        #[pygetset]
        fn u(zelf: &Py<Self>) -> (f64, f64, f64) {
            as_tuple(zelf.inner.u)
        }

        /// The `v` axis (normalized).
        #[pygetset]
        fn v(zelf: &Py<Self>) -> (f64, f64, f64) {
            as_tuple(zelf.inner.v)
        }

        /// The normal, `u × v`, normalized.
        #[pygetset]
        fn n(zelf: &Py<Self>) -> (f64, f64, f64) {
            as_tuple(zelf.inner.n)
        }

        /// Where this frame came from.
        #[pygetset]
        fn kind(zelf: &Py<Self>) -> PyWorkplaneKind {
            PyWorkplaneKind {
                inner: zelf.inner.kind.clone(),
            }
        }

        /// The world point at local `(a, b)`.
        #[pymethod]
        fn point(zelf: &Py<Self>, a: f64, b: f64) -> (f64, f64, f64) {
            as_tuple(zelf.inner.point(a, b))
        }

        /// The local `(u, v)` of a world point.
        #[pymethod]
        fn project(zelf: &Py<Self>, point: (f64, f64, f64)) -> (f64, f64) {
            let local = zelf.inner.project(triple_f64(point));
            (local[0], local[1])
        }
    }

    impl Representable for PyWorkplane {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            Ok(format!(
                "Workplane(origin={:?}, u={:?}, v={:?}, kind={:?})",
                as_tuple(zelf.inner.origin),
                as_tuple(zelf.inner.u),
                as_tuple(zelf.inner.v),
                zelf.inner.kind
            ))
        }
    }

    // -----------------------------------------------------------------------
    // Camera
    // -----------------------------------------------------------------------

    /// A view/projection camera with a device-pixel viewport.
    #[pyattr]
    #[pyclass(name = "Camera", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone)]
    pub struct PyCamera {
        inner: gfx::Camera,
    }

    impl Constructor for PyCamera {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, args: Self::Args, vm: &VirtualMachine) -> PyResult<Self> {
            let view = args
                .args
                .first()
                .map(|value| value.clone().try_into_value::<Vec<f64>>(vm))
                .transpose()?
                .ok_or_else(|| {
                    vm.new_type_error(
                        "Camera(view, projection, size=(800, 600), device_pixel_ratio=1.0)",
                    )
                })?;
            let projection = args
                .args
                .get(1)
                .map(|value| value.clone().try_into_value::<Vec<f64>>(vm))
                .transpose()?
                .ok_or_else(|| vm.new_type_error("Camera needs a projection matrix"))?;
            let size = match args.args.get(2) {
                Some(value) => pair_u32(value, vm)?,
                None => (800, 600),
            };
            let ratio = match args.args.get(3) {
                Some(value) => value.try_float(vm)?.to_f64() as f32,
                None => 1.0,
            };
            Ok(Self {
                inner: gfx::Camera {
                    view: matrix(&view, "view", vm)?,
                    projection: gfx::Projection::Matrix(matrix(&projection, "projection", vm)?),
                    viewport: gfx::Viewport::new([size.0, size.1], ratio),
                },
            })
        }
    }

    #[pyclass(with(Constructor, Representable))]
    impl PyCamera {
        /// The identity camera (clip space == world space).
        #[pystaticmethod]
        fn identity() -> Self {
            Self {
                inner: gfx::Camera::identity(),
            }
        }

        /// An orthographic camera looking down −Z from `(0, 0, eye_z)`.
        #[pystaticmethod]
        fn orthographic(
            extent: f64,
            size: OptionalArg<PyObjectRef>,
            device_pixel_ratio: OptionalArg<f64>,
            eye_z: OptionalArg<f64>,
            vm: &VirtualMachine,
        ) -> PyResult<Self> {
            let size = match size.into_option() {
                Some(value) if !vm.is_none(&value) => pair_u32(&value, vm)?,
                _ => (800, 600),
            };
            let ratio = device_pixel_ratio.into_option().unwrap_or(1.0) as f32;
            let eye_z = eye_z.into_option().unwrap_or(100.0);
            let view =
                Placement::new(Vector3::new(0.0, 0.0, -eye_z), Rotation::identity()).to_matrix();
            Ok(Self {
                inner: gfx::Camera {
                    view,
                    projection: gfx::Projection::Orthographic {
                        extent,
                        near: -1.0e4,
                        far: 1.0e4,
                    },
                    viewport: gfx::Viewport::new([size.0, size.1], ratio),
                },
            })
        }

        /// A 45°-by-default perspective camera at `(0, 0, eye_z)`.
        #[pystaticmethod]
        fn perspective(
            eye_z: f64,
            size: OptionalArg<PyObjectRef>,
            device_pixel_ratio: OptionalArg<f64>,
            fov_deg: OptionalArg<f64>,
            near: OptionalArg<f64>,
            far: OptionalArg<f64>,
            vm: &VirtualMachine,
        ) -> PyResult<Self> {
            let size = match size.into_option() {
                Some(value) if !vm.is_none(&value) => pair_u32(&value, vm)?,
                _ => (800, 600),
            };
            let ratio = device_pixel_ratio.into_option().unwrap_or(1.0) as f32;
            let fov_deg = fov_deg.into_option().unwrap_or(45.0);
            let near = near.into_option().unwrap_or(0.1);
            let far = far.into_option().unwrap_or(100.0);
            let view =
                Placement::new(Vector3::new(0.0, 0.0, -eye_z), Rotation::identity()).to_matrix();
            Ok(Self {
                inner: gfx::Camera {
                    view,
                    projection: gfx::Projection::Perspective {
                        fov_y: fov_deg.to_radians(),
                        near,
                        far,
                    },
                    viewport: gfx::Viewport::new([size.0, size.1], ratio),
                },
            })
        }

        /// The device-pixel viewport size.
        #[pygetset]
        fn size(zelf: &Py<Self>) -> (u32, u32) {
            let [w, h] = zelf.inner.viewport.device_size();
            (w, h)
        }

        /// Whether the projection is orthographic.
        #[pygetset]
        fn is_orthographic(zelf: &Py<Self>) -> bool {
            zelf.inner.is_orthographic()
        }

        /// The projection kind: `"perspective"`, `"orthographic"` or `"matrix"`.
        #[pygetset]
        fn projection_kind(zelf: &Py<Self>) -> &'static str {
            match zelf.inner.projection {
                gfx::Projection::Perspective { .. } => "perspective",
                gfx::Projection::Orthographic { .. } => "orthographic",
                gfx::Projection::Matrix(_) => "matrix",
            }
        }

        /// The eye position in world space.
        #[pygetset]
        fn position(zelf: &Py<Self>) -> (f64, f64, f64) {
            as_tuple(fs::screen::camera_position(&zelf.inner.view))
        }

        /// The world direction the camera looks along.
        #[pygetset]
        fn direction(zelf: &Py<Self>) -> (f64, f64, f64) {
            as_tuple(fs::screen::camera_forward(&zelf.inner))
        }

        /// The camera's screen-right direction in world space.
        #[pygetset]
        fn right(zelf: &Py<Self>) -> (f64, f64, f64) {
            as_tuple(fs::screen::camera_right(&zelf.inner))
        }

        /// The camera's screen-up direction in world space.
        #[pygetset]
        fn up(zelf: &Py<Self>) -> (f64, f64, f64) {
            as_tuple(fs::screen::camera_up(&zelf.inner))
        }

        /// The camera's orientation as a `(x, y, z, w)` quaternion.
        #[pygetset]
        fn orientation(zelf: &Py<Self>) -> (f64, f64, f64, f64) {
            let right = fs::screen::camera_right(&zelf.inner);
            let up = fs::screen::camera_up(&zelf.inner);
            let back = right.cross(&up);
            quaternion(right, up, back)
        }

        /// The distance from the eye to the pivot.
        #[pygetset]
        fn focal_distance(zelf: &Py<Self>) -> f64 {
            zelf.inner.eye_distance()
        }

        /// The world height of the view at the focal distance.
        #[pygetset]
        fn height(zelf: &Py<Self>) -> f64 {
            match zelf.inner.projection {
                gfx::Projection::Perspective { fov_y, .. } => {
                    2.0 * zelf.inner.eye_distance() * (fov_y / 2.0).tan()
                }
                gfx::Projection::Orthographic { extent, .. } => 2.0 * extent,
                gfx::Projection::Matrix(_) => 0.0,
            }
        }

        /// The vertical field of view in radians (orthographic reports `0`).
        #[pygetset]
        fn height_angle(zelf: &Py<Self>) -> f64 {
            match zelf.inner.projection {
                gfx::Projection::Perspective { fov_y, .. } => fov_y,
                _ => 0.0,
            }
        }

        /// The viewport aspect ratio.
        #[pygetset]
        fn aspect_ratio(zelf: &Py<Self>) -> f64 {
            zelf.inner.aspect()
        }

        /// The world → view matrix (16 row-major floats).
        #[pygetset]
        fn view_matrix(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            list_of(zelf.inner.view.values().iter().copied(), vm)
        }

        /// The view → clip matrix (16 row-major floats).
        #[pygetset]
        fn projection_matrix(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            list_of(zelf.inner.projection_matrix().values().iter().copied(), vm)
        }

        /// Project a world point to device pixels (bottom-left origin).
        #[pymethod]
        fn project(zelf: &Py<Self>, point: (f64, f64, f64)) -> Option<(f64, f64)> {
            fs::screen::project_device(&zelf.inner, triple_f64(point)).map(|[x, y]| (x, y))
        }

        /// The near and far world points of the ray through normalized viewport
        /// coordinates `(u, v)`.
        #[pymethod]
        fn ray(zelf: &Py<Self>, u: f64, v: f64) -> Option<((f64, f64, f64), (f64, f64, f64))> {
            let inverse = zelf.inner.view_projection().inverse()?;
            let x = u * 2.0 - 1.0;
            let y = v * 2.0 - 1.0;
            Some((
                as_tuple(unproject(&inverse, x, y, 0.0)),
                as_tuple(unproject(&inverse, x, y, 1.0)),
            ))
        }

        /// The ray through a device-pixel position.
        #[pymethod]
        fn ray_at(zelf: &Py<Self>, x: i32, y: i32) -> Option<((f64, f64, f64), (f64, f64, f64))> {
            let ray = zelf.inner.ray_through([x, y])?;
            Some((
                (ray.origin[0], ray.origin[1], ray.origin[2]),
                (ray.direction[0], ray.direction[1], ray.direction[2]),
            ))
        }
    }

    impl Representable for PyCamera {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            let kind = match zelf.inner.projection {
                gfx::Projection::Perspective { .. } => "perspective",
                gfx::Projection::Orthographic { .. } => "orthographic",
                gfx::Projection::Matrix(_) => "matrix",
            };
            Ok(format!(
                "Camera(position={:?}, size={:?}, {kind})",
                as_tuple(fs::screen::camera_position(&zelf.inner.view)),
                zelf.inner.viewport.device_size(),
            ))
        }
    }

    /// Unproject an NDC point with the inverse view-projection.
    fn unproject(inverse: &ferrocad_core::geometry::Matrix4, x: f64, y: f64, z: f64) -> Vector3 {
        let m = &inverse.m;
        let out = [
            m[0] * x + m[1] * y + m[2] * z + m[3],
            m[4] * x + m[5] * y + m[6] * z + m[7],
            m[8] * x + m[9] * y + m[10] * z + m[11],
            m[12] * x + m[13] * y + m[14] * z + m[15],
        ];
        let w = if out[3] != 0.0 { out[3] } else { 1.0 };
        Vector3::new(out[0] / w, out[1] / w, out[2] / w)
    }

    /// The `(x, y, z, w)` quaternion of the rotation whose world columns are
    /// `right`, `up` and `back`.
    fn quaternion(right: Vector3, up: Vector3, back: Vector3) -> (f64, f64, f64, f64) {
        let r = [
            [right.x, up.x, back.x],
            [right.y, up.y, back.y],
            [right.z, up.z, back.z],
        ];
        let trace = r[0][0] + r[1][1] + r[2][2];
        let (w, x, y, z) = if trace > 0.0 {
            let s = (trace + 1.0).sqrt() * 2.0;
            (
                0.25 * s,
                (r[2][1] - r[1][2]) / s,
                (r[0][2] - r[2][0]) / s,
                (r[1][0] - r[0][1]) / s,
            )
        } else if r[0][0] > r[1][1] && r[0][0] > r[2][2] {
            let s = (1.0 + r[0][0] - r[1][1] - r[2][2]).sqrt() * 2.0;
            (
                0.25 * s,
                (r[1][0] + r[0][1]) / s,
                (r[0][2] + r[2][0]) / s,
                (r[2][1] - r[1][2]) / s,
            )
        } else if r[1][1] > r[2][2] {
            let s = (1.0 + r[1][1] - r[0][0] - r[2][2]).sqrt() * 2.0;
            (
                0.25 * s,
                (r[1][0] + r[0][1]) / s,
                (r[2][1] + r[1][2]) / s,
                (r[0][2] - r[2][0]) / s,
            )
        } else {
            let s = (1.0 + r[2][2] - r[0][0] - r[1][1]).sqrt() * 2.0;
            (
                0.25 * s,
                (r[0][2] + r[2][0]) / s,
                (r[2][1] + r[1][2]) / s,
                (r[1][0] - r[0][1]) / s,
            )
        };
        (x, y, z, w)
    }

    // -----------------------------------------------------------------------
    // The transform gizmo
    // -----------------------------------------------------------------------

    /// A gizmo drag tick.
    #[pyattr]
    #[pyclass(name = "GizmoDragEvent", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone)]
    pub struct PyGizmoDragEvent {
        inner: fs::GizmoDragEvent,
    }

    #[pyclass]
    impl PyGizmoDragEvent {
        /// The targeted entity id.
        #[pygetset]
        fn target_id(zelf: &Py<Self>) -> u32 {
            zelf.inner.target_id
        }

        /// The handle being dragged.
        #[pygetset]
        fn active_handle(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            PyGizmoHandle {
                inner: zelf.inner.active_handle,
            }
            .into_ref(&vm.ctx)
            .into()
        }

        /// The projected numeric readout.
        #[pygetset]
        fn readout(zelf: &Py<Self>) -> String {
            zelf.inner.value.readout()
        }

        /// The transform delta since the previous tick (16 row-major floats).
        #[pygetset]
        fn delta_transform(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            list_of(zelf.inner.delta_transform.values().iter().copied(), vm)
        }

        /// The transform accumulated since the drag began.
        #[pygetset]
        fn total_transform(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            list_of(zelf.inner.total_transform.values().iter().copied(), vm)
        }
    }

    /// A gizmo drag commit (on release).
    #[pyattr]
    #[pyclass(name = "GizmoCommitEvent", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone)]
    pub struct PyGizmoCommitEvent {
        inner: fs::GizmoCommitEvent,
    }

    #[pyclass]
    impl PyGizmoCommitEvent {
        /// The targeted entity id.
        #[pygetset]
        fn target_id(zelf: &Py<Self>) -> u32 {
            zelf.inner.target_id
        }

        /// The final transform to commit to the document.
        #[pygetset]
        fn total_transform(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            list_of(zelf.inner.total_transform.values().iter().copied(), vm)
        }

        /// The final numeric readout.
        #[pygetset]
        fn readout(zelf: &Py<Self>) -> String {
            zelf.inner.value.readout()
        }
    }

    /// An interactive transform triad.
    #[pyattr]
    #[pyclass(name = "TransformGizmo", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload)]
    pub struct PyTransformGizmo {
        inner: RefCell<fs::TransformGizmo>,
    }

    impl Constructor for PyTransformGizmo {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, args: Self::Args, vm: &VirtualMachine) -> PyResult<Self> {
            let target_id = args
                .args
                .first()
                .map(|value| value.clone().try_into_value::<u32>(vm))
                .transpose()?
                .ok_or_else(|| vm.new_type_error("TransformGizmo(target_id, mode=…, origin=…)"))?;
            let mode = args
                .args
                .get(1)
                .and_then(|value| value.downcast_ref::<PyGizmoMode>())
                .map(|mode| mode.inner)
                .unwrap_or(fs::GizmoMode::Universal);
            let origin = match args.args.get(2) {
                Some(value) => point_of(value, vm)?,
                None => Vector3::zero(),
            };
            Ok(Self {
                inner: RefCell::new(fs::TransformGizmo::new(target_id, origin, mode)),
            })
        }
    }

    #[pyclass(with(Constructor))]
    impl PyTransformGizmo {
        /// The targeted entity id.
        #[pygetset]
        fn target_id(zelf: &Py<Self>) -> u32 {
            zelf.inner.borrow().target_id()
        }

        /// The gizmo's origin.
        #[pygetset]
        fn origin(zelf: &Py<Self>) -> (f64, f64, f64) {
            as_tuple(zelf.inner.borrow().origin())
        }

        #[pygetset(setter)]
        fn set_origin(zelf: &Py<Self>, origin: PyObjectRef, vm: &VirtualMachine) -> PyResult<()> {
            zelf.inner.borrow_mut().set_origin(point_of(&origin, vm)?);
            Ok(())
        }

        /// The current mode.
        #[pygetset]
        fn mode(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            PyGizmoMode {
                inner: zelf.inner.borrow().mode(),
            }
            .into_ref(&vm.ctx)
            .into()
        }

        #[pygetset(setter)]
        fn set_mode(zelf: &Py<Self>, mode: PyRef<PyGizmoMode>) {
            zelf.inner.borrow_mut().set_mode(mode.inner);
        }

        /// The current world radius (`0` before the first `update`).
        #[pygetset]
        fn scale(zelf: &Py<Self>) -> f64 {
            zelf.inner.borrow().scale()
        }

        /// The handle under the pointer, if any.
        #[pygetset]
        fn hovered(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            match zelf.inner.borrow().hovered() {
                Some(handle) => PyGizmoHandle { inner: handle }.into_ref(&vm.ctx).into(),
                None => vm.ctx.none(),
            }
        }

        /// Whether a drag is in progress.
        #[pygetset]
        fn is_dragging(zelf: &Py<Self>) -> bool {
            zelf.inner.borrow().is_dragging()
        }

        /// Highlight a handle (or clear).
        #[pymethod]
        fn set_hovered(zelf: &Py<Self>, handle: OptionalArg<PyRef<PyGizmoHandle>>) -> bool {
            let handle = handle.into_option().map(|handle| handle.inner);
            zelf.inner.borrow_mut().set_hovered(handle)
        }

        /// Recompute the screen-invariant scale for `camera`.
        #[pymethod]
        fn update(zelf: &Py<Self>, camera: PyRef<PyCamera>) -> bool {
            zelf.inner.borrow_mut().update(&camera.inner)
        }

        /// Begin a drag on `handle` at the device-pixel `position`.
        #[pymethod]
        fn begin_drag(
            zelf: &Py<Self>,
            handle: PyRef<PyGizmoHandle>,
            camera: PyRef<PyCamera>,
            position: (i32, i32),
        ) -> bool {
            zelf.inner.borrow_mut().begin_drag(
                handle.inner,
                &camera.inner,
                [position.0, position.1],
            )
        }

        /// Advance the active drag to `position`.
        #[pymethod]
        fn drag_to(
            zelf: &Py<Self>,
            camera: PyRef<PyCamera>,
            position: (i32, i32),
        ) -> Option<PyGizmoDragEvent> {
            zelf.inner
                .borrow_mut()
                .drag_to(&camera.inner, [position.0, position.1])
                .map(|inner| PyGizmoDragEvent { inner })
        }

        /// End the active drag, returning the commit.
        #[pymethod]
        fn end_drag(zelf: &Py<Self>) -> Option<PyGizmoCommitEvent> {
            zelf.inner
                .borrow_mut()
                .end_drag()
                .map(|inner| PyGizmoCommitEvent { inner })
        }

        /// Dispatch a pointer event.
        #[pymethod]
        fn handle_event(
            zelf: &Py<Self>,
            camera: PyRef<PyCamera>,
            event: PyRef<PyPointerEvent>,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            let mut gizmo = zelf.inner.borrow_mut();
            match event.inner.kind {
                fs::PointerKind::Down => {
                    if let Some(handle) = gizmo.hovered() {
                        gizmo.begin_drag(handle, &camera.inner, event.inner.position);
                    }
                    Ok(vm.ctx.none())
                }
                fs::PointerKind::Move => match gizmo.drag_to(&camera.inner, event.inner.position) {
                    Some(drag) => Ok(PyGizmoDragEvent { inner: drag }.into_ref(&vm.ctx).into()),
                    None => Ok(vm.ctx.none()),
                },
                fs::PointerKind::Up => match gizmo.end_drag() {
                    Some(commit) => Ok(PyGizmoCommitEvent { inner: commit }
                        .into_ref(&vm.ctx)
                        .into()),
                    None => Ok(vm.ctx.none()),
                },
            }
        }
    }

    // -----------------------------------------------------------------------
    // Tools
    // -----------------------------------------------------------------------

    /// An interactive tool, for Python subclassing.
    #[pyattr]
    #[pyclass(name = "Tool", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload)]
    pub struct PyTool {
        items: RefCell<Vec<fs::ShapeItem>>,
    }

    impl Constructor for PyTool {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, _args: Self::Args, _vm: &VirtualMachine) -> PyResult<Self> {
            Ok(Self {
                items: RefCell::new(Vec::new()),
            })
        }
    }

    #[pyclass(with(Constructor), flags(BASETYPE, HAS_DICT))]
    impl PyTool {
        /// React to a pointer event. **Override this in a subclass.**
        #[pymethod]
        fn on_event(_zelf: &Py<Self>, _event: PyRef<PyPointerEvent>) -> bool {
            false
        }

        /// Append a transient visual.
        #[pymethod]
        fn add_item(zelf: &Py<Self>, item: PyRef<PyShapeItem>) {
            zelf.add(item.item.borrow().clone());
        }

        /// Replace the transient visuals.
        #[pymethod]
        fn set_items(zelf: &Py<Self>, items: Vec<PyObjectRef>) {
            let mut slot = zelf.items.borrow_mut();
            slot.clear();
            for item in items {
                if let Some(item) = item.downcast_ref::<PyShapeItem>() {
                    slot.push(item.item.borrow().clone());
                }
            }
        }

        /// Drop the transient visuals.
        #[pymethod]
        fn clear_items(zelf: &Py<Self>) {
            zelf.items.borrow_mut().clear();
        }

        /// The transient visuals (as copies).
        #[pygetset]
        fn items(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            let items: Vec<PyObjectRef> = zelf
                .items
                .borrow()
                .iter()
                .map(|item| PyShapeItem::of(item.clone()).into_ref(&vm.ctx).into())
                .collect();
            vm.ctx.new_list(items).into()
        }
    }

    impl PyTool {
        fn add(&self, item: fs::ShapeItem) {
            self.items.borrow_mut().push(item);
        }
    }

    /// A Python `Tool` instance driven through the native [`fs::Tool`] interface.
    pub(super) struct PyToolAdapter {
        interpreter: Rc<Interpreter>,
        object: PyObjectRef,
        items: Vec<fs::ShapeItem>,
        status: fs::ToolStatus,
    }

    impl PyToolAdapter {
        fn new(interpreter: Rc<Interpreter>, object: PyObjectRef) -> PyResult<Self> {
            let (items, status) = interpreter.enter(|vm| {
                if object.downcast_ref::<PyTool>().is_none() {
                    return Err(vm.new_type_error("expected a `ferrocad_stage.Tool` instance"));
                }
                Ok((read_items(&object), read_status(&object, vm)))
            })?;
            Ok(Self {
                interpreter,
                object,
                items,
                status,
            })
        }

        fn refresh(&mut self) {
            let object = self.object.clone();
            self.interpreter.enter(|vm| {
                self.items = read_items(&object);
                self.status = read_status(&object, vm);
            });
        }
    }

    /// The items a Python tool currently holds.
    fn read_items(object: &PyObjectRef) -> Vec<fs::ShapeItem> {
        object
            .downcast_ref::<PyTool>()
            .map(|tool| tool.items.borrow().clone())
            .unwrap_or_default()
    }

    /// The live status from a Python `status()` method, when the tool has one.
    fn read_status(object: &PyObjectRef, vm: &VirtualMachine) -> fs::ToolStatus {
        let Ok(value) = object
            .get_attr(vm.ctx.intern_str("status"), vm)
            .and_then(|method| method.call((), vm))
        else {
            return fs::ToolStatus::inactive();
        };
        let Ok(status) = value.downcast::<PyDict>() else {
            return fs::ToolStatus::inactive();
        };
        let string = |key: &str| -> String {
            status
                .get_item_opt(key, vm)
                .ok()
                .flatten()
                .and_then(|value| {
                    value
                        .str(vm)
                        .ok()
                        .and_then(|text| text.to_str().map(str::to_string))
                })
                .unwrap_or_default()
        };
        fs::ToolStatus {
            id: string("id"),
            label: string("label"),
            active: status
                .get_item_opt("active", vm)
                .ok()
                .flatten()
                .map(|value| value.is_true(vm).unwrap_or(false))
                .unwrap_or(false),
            points: status
                .get_item_opt("points", vm)
                .ok()
                .flatten()
                .and_then(|value| value.clone().try_into_value::<usize>(vm).ok())
                .unwrap_or(0),
            hint: string("hint"),
        }
    }

    impl fs::Tool for PyToolAdapter {
        fn on_event(&mut self, event: &fs::PointerEvent, snap: Option<fs::Snap>) {
            let event = PyPointerEvent {
                inner: *event,
                snap: snap.map(|snap| snap.point),
            };
            let object = self.object.clone();
            // A raising handler is reported, not propagated: one broken tool must not
            // take down the frame loop.
            self.interpreter.enter(|vm| {
                let event: PyObjectRef = event.into_ref(&vm.ctx).into();
                let handler = object.get_attr(vm.ctx.intern_str("on_event"), vm);
                match handler {
                    Ok(handler) => {
                        if let Err(error) = handler.call((event,), vm) {
                            vm.print_exception(&error);
                        }
                    }
                    Err(error) => vm.print_exception(&error),
                }
                self.items = read_items(&object);
                self.status = read_status(&object, vm);
            });
        }

        fn items(&self) -> &[fs::ShapeItem] {
            &self.items
        }

        fn status(&self) -> fs::ToolStatus {
            self.status.clone()
        }

        fn cancel(&mut self) -> bool {
            let object = self.object.clone();
            let cancelled = self.interpreter.enter(|vm| {
                let method = object.get_attr(vm.ctx.intern_str("cancel"), vm);
                match method {
                    Ok(method) => match method.call((), vm) {
                        Ok(value) => value.is_true(vm).unwrap_or(true),
                        Err(error) => {
                            vm.print_exception(&error);
                            false
                        }
                    },
                    Err(_) => false,
                }
            });
            self.refresh();
            cancelled
        }
    }

    /// Wrap a Python `Tool` instance in the native [`fs::Tool`] interface.
    pub(crate) fn extract_tool(
        interpreter: Rc<Interpreter>,
        object: PyObjectRef,
    ) -> PyResult<Box<dyn fs::Tool>> {
        Ok(Box::new(PyToolAdapter::new(interpreter, object)?))
    }

    // -----------------------------------------------------------------------
    // Picking and the published view
    // -----------------------------------------------------------------------

    /// One ranked pick (`ferrocad_stage::view::RankedHit`).
    #[pyattr]
    #[pyclass(name = "PickHit", module = "ferrocad_stage")]
    #[derive(Debug, PyPayload, Clone)]
    pub struct PyPickHit {
        inner: fs::view::RankedHit,
    }

    #[pyclass]
    impl PyPickHit {
        #[pygetset]
        fn entity_id(zelf: &Py<Self>) -> u32 {
            zelf.inner.entity_id
        }

        #[pygetset]
        fn part_idx(zelf: &Py<Self>) -> u32 {
            zelf.inner.part_idx
        }

        #[pygetset]
        fn part(zelf: &Py<Self>) -> Option<String> {
            zelf.inner.part.clone()
        }

        #[pygetset]
        fn object(zelf: &Py<Self>) -> Option<String> {
            zelf.inner.object.clone()
        }

        #[pygetset]
        fn point(zelf: &Py<Self>) -> (f32, f32, f32) {
            triple(zelf.inner.point)
        }

        #[pygetset]
        fn normal(zelf: &Py<Self>) -> (f32, f32, f32) {
            triple(zelf.inner.normal)
        }

        #[pygetset]
        fn distance(zelf: &Py<Self>) -> f64 {
            zelf.inner.distance
        }

        #[pygetset]
        fn topology(zelf: &Py<Self>) -> &'static str {
            match zelf.inner.topology {
                gfx::Topology::Triangles => "Triangles",
                gfx::Topology::Lines => "Lines",
                gfx::Topology::Points => "Points",
            }
        }

        #[pygetset]
        fn element(zelf: &Py<Self>) -> u32 {
            zelf.inner.element
        }
    }

    /// The ranked hits under device position `(x, y)` (bottom-left origin).
    #[pyfunction]
    fn pick_at(x: i32, y: i32, max_hits: OptionalArg<usize>, vm: &VirtualMachine) -> PyObjectRef {
        let hits = fs::view::ranked_pick([x, y], max_hits.into_option().unwrap_or(0));
        list_of(
            hits.into_iter()
                .map(|inner| -> PyObjectRef { PyPickHit { inner }.into_ref(&vm.ctx).into() }),
            vm,
        )
    }

    /// Publish the camera the facade reports.
    #[pyfunction]
    fn set_view_camera(camera: PyRef<PyCamera>) {
        fs::view::publish_camera(camera.inner.clone());
    }

    /// Publish the stage a pick walks, with the name tables it reports.
    #[pyfunction]
    fn set_view_stage(
        stage: PyRef<PyStage>,
        objects: OptionalArg<PyObjectRef>,
        parts: OptionalArg<PyObjectRef>,
        vm: &VirtualMachine,
    ) -> PyResult<()> {
        let objects = match objects.into_option() {
            Some(value) if !vm.is_none(&value) => dict_strings(&value, vm)?,
            _ => HashMap::new(),
        };
        let parts = match parts.into_option() {
            Some(value) if !vm.is_none(&value) => Some(dict_names(&value, vm)?),
            _ => None,
        };
        fs::view::publish_stage(&stage.inner.borrow(), objects, parts);
        Ok(())
    }

    /// `{pick id: name}` from a Python dict.
    fn dict_strings(value: &PyObjectRef, vm: &VirtualMachine) -> PyResult<HashMap<u32, String>> {
        let Some(dict) = value.downcast_ref::<PyDict>() else {
            return Err(vm.new_type_error("expected a dict"));
        };
        let mut out = HashMap::new();
        for (key, value) in dict.items_vec() {
            let name = value
                .str(vm)
                .ok()
                .and_then(|text| text.to_str().map(str::to_string))
                .unwrap_or_default();
            out.insert(key.clone().try_into_value::<u32>(vm)?, name);
        }
        Ok(out)
    }

    /// `{pick id: [name, …]}` from a Python dict.
    fn dict_names(value: &PyObjectRef, vm: &VirtualMachine) -> PyResult<HashMap<u32, Vec<String>>> {
        let Some(dict) = value.downcast_ref::<PyDict>() else {
            return Err(vm.new_type_error("expected a dict"));
        };
        let mut out = HashMap::new();
        for (key, value) in dict.items_vec() {
            let names = value
                .clone()
                .try_into_value::<Vec<PyObjectRef>>(vm)?
                .into_iter()
                .map(|name| {
                    name.str(vm)
                        .ok()
                        .and_then(|text| text.to_str().map(str::to_string))
                        .unwrap_or_default()
                })
                .collect();
            out.insert(key.clone().try_into_value::<u32>(vm)?, names);
        }
        Ok(out)
    }

    /// The published camera, or `None` when no host has published one.
    #[pyfunction]
    fn view_camera(vm: &VirtualMachine) -> PyObjectRef {
        match fs::view::published_camera() {
            Some(camera) => PyCamera { inner: camera }.into_ref(&vm.ctx).into(),
            None => vm.ctx.none(),
        }
    }

    /// Forget the published camera and stage.
    #[pyfunction]
    fn clear_view() {
        fs::view::clear_view();
    }

    /// Publish the transient preview items a script wants drawn.
    #[pyfunction]
    fn set_preview_items(items: Vec<PyObjectRef>) -> u64 {
        let items: Vec<fs::ShapeItem> = items
            .iter()
            .filter_map(|item| item.downcast_ref::<PyShapeItem>())
            .map(|item| item.item.borrow().clone())
            .collect();
        fs::view::set_preview_items(items)
    }

    /// The published transient preview items.
    #[pyfunction]
    fn preview_items(vm: &VirtualMachine) -> PyObjectRef {
        let items: Vec<PyObjectRef> = fs::view::preview_items()
            .into_iter()
            .map(|item| PyShapeItem::of(item).into_ref(&vm.ctx).into())
            .collect();
        vm.ctx.new_list(items).into()
    }

    /// Drop every transient preview.
    #[pyfunction]
    fn clear_previews() -> u64 {
        fs::view::clear_previews()
    }

    /// A counter that changes whenever the preview set does.
    #[pyfunction]
    fn preview_generation() -> u64 {
        fs::view::preview_generation()
    }
}
