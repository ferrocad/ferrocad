//! `pivy.coin` for RustPython, over the same `ferrocad_inventor` the CPython facade binds.
//!
//! This is an **adapter**, not a second implementation: every rule lives below it.
//! [`ferrocad_inventor::fields`] owns the name→buffer table and Coin's
//! `set("field VALUE")` parser; [`ferrocad_inventor::class_name`] owns the class names;
//! [`ferrocad_inventor::view_root`] owns the `RootNode` tree; and
//! [`ferrocad_inventor::overlays`] owns the pull registry the host drains. What is here
//! is what a Python object needs: the class spine, the field object's method surface, and
//! the argument conversions.
//!
//! The surface is the subset the shipped Coin workbenches use (`mods/GridOverlayCoin`,
//! `mods/AxisOverlayCoin`, `mods/GizmoOverlayShader`) plus everything
//! `ViewObject.RootNode` publishes. Deliberate cuts, stated rather than faked:
//!
//! - no `SoType.fromName(...).createInstance()` (nothing shipped reaches a node class by
//!   name on this runtime), and no camera/event classes;
//! - a field answers the method surface (`getNum`/`getValues`/`setValues`/`setValue`/
//!   `set1Value`/`setNum`/`append`), not the sequence protocol: RustPython's length and
//!   indexing slots are `AsSequence`, and no shipped mod indexes a field;
//! - a node list (`shaderObject`, `parameter`) stores copies, exactly as `addChild` does
//!   here and in the CPython facade.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use ferrocad_inventor::fields::{self as core, FieldId};
use ferrocad_inventor::{
    Node, NodeKind, Scene, ShaderParameter, ShaderStage, ShaderStageKind, Topology,
    depth_function, light_model, material_binding, pick_style,
};
use rustpython_vm::builtins::{PyStrRef, PyType};
use rustpython_vm::function::{FuncArgs, OptionalArg};
use rustpython_vm::types::{Constructor, Representable};
use rustpython_vm::{
    Context, Py, PyObjectRef, PyPayload, PyRef, PyResult, VirtualMachine, pyclass, pymodule,
};

pub(crate) use coin::module_def as coin_module_def;
pub(crate) use coin::root_py as root_node_py;

#[pymodule(name = "coin")]
mod coin {
    #![allow(non_snake_case, non_upper_case_globals)]

    use super::*;

    // -----------------------------------------------------------------------
    // The node
    // -----------------------------------------------------------------------

    /// A scene-graph node: the core tree, shared with every field view of it.
    ///
    /// The `Arc` is what lets a field object outlive the call that produced it —
    /// `SoCoordinate3().point` keeps writing to the node it came from, as Coin's live
    /// fields do.
    #[pyattr]
    #[pyclass(name = "SoNode", module = "coin")]
    #[derive(Debug, PyPayload)]
    pub struct PyNode {
        inner: Arc<Mutex<Node>>,
    }

    impl PyNode {
        fn of(node: Node) -> Self {
            Self {
                inner: Arc::new(Mutex::new(node)),
            }
        }

        /// The tree, cloned.
        fn snapshot(&self, vm: &VirtualMachine) -> PyResult<Node> {
            self.inner
                .lock()
                .map(|node| node.clone())
                .map_err(|_| vm.new_runtime_error("coin: the node is busy"))
        }

        /// Run `f` on the tree.
        fn with<R>(&self, vm: &VirtualMachine, f: impl FnOnce(&mut Node) -> R) -> PyResult<R> {
            let mut node = self
                .inner
                .lock()
                .map_err(|_| vm.new_runtime_error("coin: the node is busy"))?;
            Ok(f(&mut node))
        }
    }

    #[pyclass(with(Representable))]
    impl PyNode {
        /// `node.set("field VALUE")` — Coin's `SoFieldContainer::set`.
        #[pymethod]
        fn set(zelf: &Py<Self>, spec: String, vm: &VirtualMachine) -> PyResult<()> {
            zelf.with(vm, |node| core::set_from_text(node, &spec))
                .and_then(|result| {
                    result.map_err(|error| vm.new_value_error(error.to_string()))
                })
        }
    }

    impl Representable for PyNode {
        fn repr_str(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<String> {
            let node = zelf
                .inner
                .lock()
                .map_err(|_| vm.new_runtime_error("coin: the node is busy"))?;
            Ok(format!(
                "<{} object>",
                ferrocad_inventor::class_name(&node.kind)
            ))
        }
    }

    // -----------------------------------------------------------------------
    // The class spine
    // -----------------------------------------------------------------------

    /// A group of children (Coin's `SoGroup`); `addChild` copies the child's tree.
    #[pyattr]
    #[pyclass(name = "SoGroup", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyGroup(PyNode);

    #[pyclass]
    impl PyGroup {
        /// `group.addChild(child)` — appends a copy of `child`'s node.
        #[pymethod]
        fn addChild(zelf: &Py<Self>, child: PyRef<PyNode>, vm: &VirtualMachine) -> PyResult<()> {
            let child = child.snapshot(vm)?;
            zelf.0.with(vm, |node| node.children.push(child))
        }

        /// `group.removeChild(node)` — removes the first structurally equal child.
        #[pymethod]
        fn removeChild(zelf: &Py<Self>, child: PyRef<PyNode>, vm: &VirtualMachine) -> PyResult<()> {
            let target = child.snapshot(vm)?;
            zelf.0.with(vm, |node| {
                if let Some(index) = node
                    .children
                    .iter()
                    .position(|candidate| *candidate == target)
                {
                    node.children.remove(index);
                }
            })
        }

        /// `group.getNumChildren()`.
        #[pymethod]
        fn getNumChildren(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<usize> {
            zelf.0.with(vm, |node| node.children.len())
        }
    }

    const RENDER_CULLING_OFF: i32 = 0;
    const RENDER_CULLING_ON: i32 = 1;
    const RENDER_CULLING_AUTO: i32 = 2;

    /// Groups children and isolates state (Coin's `SoSeparator`).
    #[pyattr]
    #[pyclass(name = "SoSeparator", module = "coin", base = PyGroup)]
    #[repr(C)]
    #[derive(Debug)]
    /// The second field is `renderCulling`, accepted and stored but not applied
    /// (FerroCAD draws every packet), as the CPython facade stores it.
    pub struct PySeparator(PyGroup, AtomicI32);

    impl Constructor for PySeparator {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, _args: Self::Args, _vm: &VirtualMachine) -> PyResult<Self> {
            Ok(Self(
                PyGroup(PyNode::of(Node {
                    kind: NodeKind::Separator,
                    children: Vec::new(),
                })),
                AtomicI32::new(RENDER_CULLING_AUTO),
            ))
        }
    }

    #[pyclass(with(Constructor))]
    impl PySeparator {
        #[extend_class]
        fn extend_class(ctx: &Context, class: &'static Py<PyType>) {
            for (name, value) in [
                ("OFF", RENDER_CULLING_OFF),
                ("ON", RENDER_CULLING_ON),
                ("AUTO", RENDER_CULLING_AUTO),
            ] {
                class.set_str_attr(name, ctx.new_int(value), ctx);
            }
        }

        #[pygetset]
        fn renderCulling(zelf: &Py<Self>) -> i32 {
            zelf.1.load(Ordering::Relaxed)
        }

        #[pygetset(setter)]
        fn set_renderCulling(zelf: &Py<Self>, value: i32) {
            zelf.1.store(value, Ordering::Relaxed);
        }
    }

    /// Draws only the selected child (Coin's `SoSwitch`).
    #[pyattr]
    #[pyclass(name = "SoSwitch", module = "coin", base = PyGroup)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PySwitch(PyGroup);

    impl Constructor for PySwitch {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, _args: Self::Args, _vm: &VirtualMachine) -> PyResult<Self> {
            Ok(Self(PyGroup(PyNode::of(Node {
                kind: NodeKind::Switch { which: 0 },
                children: Vec::new(),
            }))))
        }
    }

    #[pyclass(with(Constructor))]
    impl PySwitch {
        /// `switch.whichChild` — the active index, or a `SO_SWITCH_*` sentinel.
        #[pygetset]
        fn whichChild(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<i32> {
            zelf.0.0.with(vm, |node| match node.kind {
                NodeKind::Switch { which } => which,
                _ => 0,
            })
        }

        #[pygetset(setter)]
        fn set_whichChild(zelf: &Py<Self>, value: i32, vm: &VirtualMachine) -> PyResult<()> {
            zelf.0.0.with(vm, |node| {
                if let NodeKind::Switch { which } = &mut node.kind {
                    *which = value;
                }
            })
        }
    }

    // Leaf node classes. `#[pyattr]` is a marker the `#[pymodule]` macro consumes, so
    // the structs are declared here rather than generated by a `macro_rules!`, which
    // would expand after the module did. The constructors are generated.

    /// A vertex position list (`SoCoordinate3`).
    #[pyattr]
    #[pyclass(name = "SoCoordinate3", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyCoordinate3(PyNode);

    /// A vertex normal list (`SoNormal`).
    #[pyattr]
    #[pyclass(name = "SoNormal", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyNormal(PyNode);

    /// The diffuse colour list (`SoMaterial`).
    #[pyattr]
    #[pyclass(name = "SoMaterial", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyMaterial(PyNode);

    /// A non-indexed polygon set (`SoFaceSet`).
    #[pyattr]
    #[pyclass(name = "SoFaceSet", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyFaceSet(PyNode);

    /// An indexed triangle set (`SoIndexedFaceSet`).
    #[pyattr]
    #[pyclass(name = "SoIndexedFaceSet", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyIndexedFaceSet(PyNode);

    /// An indexed line set (`SoIndexedLineSet`).
    #[pyattr]
    #[pyclass(name = "SoIndexedLineSet", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyIndexedLineSet(PyNode);

    /// A pickability hint (`SoPickStyle`).
    #[pyattr]
    #[pyclass(name = "SoPickStyle", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyPickStyle(PyNode);

    /// A shading-model hint (`SoLightModel`).
    #[pyattr]
    #[pyclass(name = "SoLightModel", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyLightModel(PyNode);

    /// Depth-buffer state (`SoDepthBuffer`).
    #[pyattr]
    #[pyclass(name = "SoDepthBuffer", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyDepthBuffer(PyNode);

    /// A material-binding hint (`SoMaterialBinding`).
    #[pyattr]
    #[pyclass(name = "SoMaterialBinding", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyMaterialBinding(PyNode);

    /// A shader program (`SoShaderProgram`).
    #[pyattr]
    #[pyclass(name = "SoShaderProgram", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyShaderProgram(PyNode);

    /// A vertex shader stage (`SoVertexShader`).
    #[pyattr]
    #[pyclass(name = "SoVertexShader", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyVertexShader(PyNode);

    /// A fragment shader stage (`SoFragmentShader`).
    #[pyattr]
    #[pyclass(name = "SoFragmentShader", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyFragmentShader(PyNode);

    /// A float shader parameter (`SoShaderParameter1f`).
    #[pyattr]
    #[pyclass(name = "SoShaderParameter1f", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyShaderParameter1f(PyNode);

    /// A 3-vector shader parameter (`SoShaderParameter3f`).
    #[pyattr]
    #[pyclass(name = "SoShaderParameter3f", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyShaderParameter3f(PyNode);

    /// A float-array shader parameter (`SoShaderParameterArray1f`).
    #[pyattr]
    #[pyclass(name = "SoShaderParameterArray1f", module = "coin", base = PyNode)]
    #[repr(transparent)]
    #[derive(Debug)]
    pub struct PyShaderParameterArray1f(PyNode);

    /// The constructor of every leaf class: one fresh node of its kind.
    macro_rules! ctor {
        ($name:ident, $kind:expr) => {
            impl Constructor for $name {
                type Args = FuncArgs;

                fn py_new(
                    _cls: &Py<PyType>,
                    _args: Self::Args,
                    _vm: &VirtualMachine,
                ) -> PyResult<Self> {
                    Ok(Self(PyNode::of(Node {
                        kind: $kind,
                        children: Vec::new(),
                    })))
                }
            }
        };
    }

    ctor!(PyCoordinate3, NodeKind::Coordinate3(Vec::new()));
    ctor!(PyNormal, NodeKind::Normal(Vec::new()));
    ctor!(PyMaterial, NodeKind::Material(Vec::new()));
    ctor!(PyFaceSet, NodeKind::FaceSet { num_vertices: Vec::new() });
    ctor!(PyIndexedFaceSet, NodeKind::Mesh { indices: Vec::new(), normal_indices: Vec::new(), topology: Topology::Triangles });
    ctor!(PyIndexedLineSet, NodeKind::Mesh { indices: Vec::new(), normal_indices: Vec::new(), topology: Topology::Lines });
    ctor!(PyPickStyle, NodeKind::PickStyle { style: pick_style::SHAPE });
    ctor!(PyLightModel, NodeKind::LightModel { model: light_model::PHONG });
    ctor!(PyDepthBuffer, NodeKind::DepthBuffer { test: true, write: true, function: depth_function::LESS });
    ctor!(PyMaterialBinding, NodeKind::MaterialBinding { value: material_binding::OVERALL });
    ctor!(PyShaderProgram, NodeKind::ShaderProgram);
    ctor!(PyVertexShader, NodeKind::ShaderStage(ShaderStage::vertex()));
    ctor!(PyFragmentShader, NodeKind::ShaderStage(ShaderStage::fragment()));
    ctor!(PyShaderParameter1f, NodeKind::ShaderParameter(ShaderParameter::Float { name: String::new(), value: 0.0 }));
    ctor!(PyShaderParameter3f, NodeKind::ShaderParameter(ShaderParameter::Vec3 { name: String::new(), value: [0.0; 3] }));
    ctor!(PyShaderParameterArray1f, NodeKind::ShaderParameter(ShaderParameter::Floats { name: String::new(), values: Vec::new() }));

    // -----------------------------------------------------------------------
    // Fields
    // -----------------------------------------------------------------------

    /// A live Coin field: the node's own buffer, addressed by name.
    ///
    /// Every operation reads the buffer out of the core, edits it, and writes it back, so
    /// `SoMFVec3f`/`SoMFColor`/`SoMFInt32`/`SoMFFloat` and `SoMFNode` are one class here.
    #[pyattr]
    #[pyclass(name = "SoField", module = "coin")]
    #[derive(Debug, PyPayload)]
    pub struct PyField {
        node: Arc<Mutex<Node>>,
        field: FieldId,
    }

    fn field_py(node: &Arc<Mutex<Node>>, field: FieldId, vm: &VirtualMachine) -> PyObjectRef {
        PyField {
            node: Arc::clone(node),
            field,
        }
        .into_ref(&vm.ctx)
        .into()
    }

    /// One field element, in the runtime-neutral shape the core speaks.
    #[derive(Clone, Copy, Debug)]
    enum Elem {
        Vec3([f32; 3]),
        Color([f32; 4]),
        Int(u32),
        Float(f32),
    }

    /// The buffer's elements, read out.
    fn read(node: &Node, field: FieldId, vm: &VirtualMachine) -> PyResult<Vec<Elem>> {
        if let Some(values) = core::vec3(node, field) {
            return Ok(values.iter().map(|v| Elem::Vec3(*v)).collect());
        }
        if let Some(values) = core::color(node, field) {
            return Ok(values.iter().map(|v| Elem::Color(*v)).collect());
        }
        if let Some(values) = core::int(node, field) {
            return Ok(values.iter().map(|v| Elem::Int(*v)).collect());
        }
        if let Some(values) = core::float(node, field) {
            return Ok(values.iter().map(|v| Elem::Float(*v)).collect());
        }
        Err(vm.new_runtime_error(format!(
            "{}: the node has no such field",
            field.label()
        )))
    }

    fn vec3_of(elem: &Elem) -> Option<[f32; 3]> {
        match elem {
            Elem::Vec3(value) => Some(*value),
            _ => None,
        }
    }

    fn color_of(elem: &Elem) -> Option<[f32; 4]> {
        match elem {
            Elem::Color(value) => Some(*value),
            _ => None,
        }
    }

    fn int_of(elem: &Elem) -> Option<u32> {
        match elem {
            Elem::Int(value) => Some(*value),
            _ => None,
        }
    }

    fn float_of(elem: &Elem) -> Option<f32> {
        match elem {
            Elem::Float(value) => Some(*value),
            _ => None,
        }
    }

    /// Write elements back into the buffer.
    fn write(
        node: &mut Node,
        field: FieldId,
        values: &[Elem],
        vm: &VirtualMachine,
    ) -> PyResult<()> {
        if let Some(buffer) = core::vec3_mut(node, field) {
            *buffer = values.iter().filter_map(vec3_of).collect();
            return Ok(());
        }
        if let Some(buffer) = core::color_mut(node, field) {
            *buffer = values.iter().filter_map(color_of).collect();
            return Ok(());
        }
        if let Some(buffer) = core::int_mut(node, field) {
            *buffer = values.iter().filter_map(int_of).collect();
            return Ok(());
        }
        if let Some(buffer) = core::float_mut(node, field) {
            *buffer = values.iter().filter_map(float_of).collect();
            return Ok(());
        }
        Err(vm.new_runtime_error(format!(
            "{}: the node has no such field",
            field.label()
        )))
    }

    /// Parse one field element in the shape the field's class declares.
    fn parse_elem(field: FieldId, value: &PyObjectRef, vm: &VirtualMachine) -> PyResult<Elem> {
        match field.label() {
            "SoMFVec3f" => Ok(Elem::Vec3(numbers3(value, vm)?)),
            "SoMFColor" => {
                let values = numbers(value, vm)?;
                Ok(Elem::Color(match values.as_slice() {
                    [r, g, b] => [*r, *g, *b, 1.0],
                    [r, g, b, a] => [*r, *g, *b, *a],
                    other => {
                        return Err(vm.new_value_error(format!(
                            "SoMFColor expects 3 or 4 components, got {}",
                            other.len()
                        )))
                    }
                }))
            }
            "SoMFInt32" => Ok(Elem::Int(value.try_float(vm)?.to_f64().max(0.0) as u32)),
            _ => Ok(Elem::Float(value.try_float(vm)?.to_f64() as f32)),
        }
    }

    fn elem_py(elem: Elem, vm: &VirtualMachine) -> PyObjectRef {
        match elem {
            Elem::Vec3(value) => tuple_py(&value, vm),
            Elem::Color(value) => tuple_py(&value, vm),
            Elem::Int(value) => vm.new_pyobj(i64::from(value)),
            Elem::Float(value) => vm.new_pyobj(f64::from(value)),
        }
    }

    fn tuple_py(values: &[f32], vm: &VirtualMachine) -> PyObjectRef {
        vm.ctx
            .new_tuple(
                values
                    .iter()
                    .map(|value| vm.new_pyobj(f64::from(*value)))
                    .collect(),
            )
            .into()
    }

    fn numbers(value: &PyObjectRef, vm: &VirtualMachine) -> PyResult<Vec<f32>> {
        let items: Vec<PyObjectRef> = value
            .clone()
            .try_into_value(vm)
            .map_err(|_| vm.new_type_error("expected a sequence of numbers"))?;
        items
            .iter()
            .map(|item| item.try_float(vm).map(|value| value.to_f64() as f32))
            .collect()
    }

    fn numbers3(value: &PyObjectRef, vm: &VirtualMachine) -> PyResult<[f32; 3]> {
        let values = numbers(value, vm)?;
        match values.as_slice() {
            [x, y, z] => Ok([*x, *y, *z]),
            other => Err(vm.new_value_error(format!(
                "SoMFVec3f expects 3 components, got {}",
                other.len()
            ))),
        }
    }

    /// Parse a whole Python sequence of elements.
    fn elems(field: FieldId, values: &PyObjectRef, vm: &VirtualMachine) -> PyResult<Vec<Elem>> {
        let items: Vec<PyObjectRef> = values
            .clone()
            .try_into_value(vm)
            .map_err(|_| vm.new_type_error("expected a sequence"))?;
        items
            .iter()
            .map(|item| parse_elem(field, item, vm))
            .collect()
    }

    /// Grow `buffer` to `len`, filling with the element default.
    fn grow(buffer: &mut Vec<Elem>, field: FieldId, len: usize) {
        while buffer.len() < len {
            buffer.push(match field.label() {
                "SoMFVec3f" => Elem::Vec3([0.0; 3]),
                "SoMFColor" => Elem::Color([0.0, 0.0, 0.0, 1.0]),
                "SoMFInt32" => Elem::Int(0),
                _ => Elem::Float(0.0),
            });
        }
    }

    /// The `Node` a Python node value holds.
    fn node_arg(value: &PyObjectRef, vm: &VirtualMachine) -> PyResult<Node> {
        value
            .downcast_ref::<PyNode>()
            .ok_or_else(|| vm.new_type_error("SoMFNode expects `coin` scene-graph nodes"))?
            .snapshot(vm)
    }

    /// A detached Python node for a stored child (value semantics, as `addChild`).
    fn node_py(node: Node, vm: &VirtualMachine) -> PyObjectRef {
        PyNode::of(node).into_ref(&vm.ctx).into()
    }

    #[pyclass(with(Representable))]
    impl PyField {
        /// `field.getNum()`.
        #[pymethod]
        fn getNum(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<usize> {
            let node = zelf
                .node
                .lock()
                .map_err(|_| vm.new_runtime_error("coin: the field is busy"))?;
            match core::nodes(&node, zelf.field) {
                Some(children) => Ok(children.len()),
                None => Ok(read(&node, zelf.field, vm)?.len()),
            }
        }

        /// `field.getValues()` — the elements as a list.
        #[pymethod]
        fn getValues(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let node = zelf
                .node
                .lock()
                .map_err(|_| vm.new_runtime_error("coin: the field is busy"))?;
            if let Some(children) = core::nodes(&node, zelf.field) {
                let items: Vec<PyObjectRef> = children
                    .iter()
                    .map(|child| node_py(child.clone(), vm))
                    .collect();
                return Ok(vm.ctx.new_list(items).into());
            }
            let values = read(&node, zelf.field, vm)?;
            let items: Vec<PyObjectRef> = values.into_iter().map(|v| elem_py(v, vm)).collect();
            Ok(vm.ctx.new_list(items).into())
        }

        /// `field.setValues(start, values)` or `field.setValues(start, num, values)`.
        #[pymethod]
        fn setValues(
            zelf: &Py<Self>,
            start: usize,
            num_or_values: PyObjectRef,
            values: OptionalArg<PyObjectRef>,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            let mut node = zelf
                .node
                .lock()
                .map_err(|_| vm.new_runtime_error("coin: the field is busy"))?;
            // A node list takes nodes: `stage.parameter.setValues(0, n, [nodes])` is how a
            // shader stage links its parameters.
            if core::nodes_mut(&mut node, zelf.field).is_some() {
                let (start, items) = match values.into_option() {
                    Some(values) => {
                        let count = num_or_values.try_float(vm)?.to_f64().max(0.0) as usize;
                        let items: Vec<PyObjectRef> = values
                            .clone()
                            .try_into_value(vm)
                            .map_err(|_| vm.new_type_error("expected a sequence of nodes"))?;
                        (start, items.into_iter().take(count).collect::<Vec<_>>())
                    }
                    None => {
                        let items: Vec<PyObjectRef> = num_or_values
                            .clone()
                            .try_into_value(vm)
                            .map_err(|_| vm.new_type_error("expected a sequence of nodes"))?;
                        (start, items)
                    }
                };
                let mut children = items
                    .iter()
                    .map(|value| node_arg(value, vm))
                    .collect::<PyResult<Vec<Node>>>()?;
                let Some(buffer) = core::nodes_mut(&mut node, zelf.field) else {
                    unreachable!("checked above")
                };
                if start > buffer.len() {
                    buffer.resize(start, Node::default());
                }
                buffer.truncate(start);
                buffer.append(&mut children);
                return Ok(());
            }
            match values.into_option() {
                // The three-argument form replaces `num` entries from `start`.
                Some(values) => {
                    let count = num_or_values.try_float(vm)?.to_f64().max(0.0) as usize;
                    let parsed: Vec<Elem> = elems(zelf.field, &values, vm)?
                        .into_iter()
                        .take(count)
                        .collect();
                    if parsed.is_empty() {
                        return Ok(());
                    }
                    let mut buffer = read(&node, zelf.field, vm)?;
                    grow(&mut buffer, zelf.field, start + parsed.len());
                    buffer[start..start + parsed.len()].copy_from_slice(&parsed);
                    write(&mut node, zelf.field, &buffer, vm)
                }
                // The two-argument form replaces the run from `start`.
                None => {
                    let parsed = elems(zelf.field, &num_or_values, vm)?;
                    let mut buffer = read(&node, zelf.field, vm)?;
                    if start > buffer.len() {
                        grow(&mut buffer, zelf.field, start);
                    }
                    buffer.truncate(start);
                    buffer.extend_from_slice(&parsed);
                    write(&mut node, zelf.field, &buffer, vm)
                }
            }
        }

        /// `field.setNum(n)` — resize, filling new elements with the default.
        #[pymethod]
        fn setNum(zelf: &Py<Self>, num: i64, vm: &VirtualMachine) -> PyResult<()> {
            if num < 0 {
                return Err(vm.new_value_error(format!(
                    "setNum: num must be non-negative, got {num}"
                )));
            }
            let mut node = zelf
                .node
                .lock()
                .map_err(|_| vm.new_runtime_error("coin: the field is busy"))?;
            let mut buffer = read(&node, zelf.field, vm)?;
            grow(&mut buffer, zelf.field, num as usize);
            write(&mut node, zelf.field, &buffer, vm)
        }

        /// `field.setValue(value)` — the singular overload: a one-element field.
        #[pymethod]
        fn setValue(zelf: &Py<Self>, value: PyObjectRef, vm: &VirtualMachine) -> PyResult<()> {
            let mut node = zelf
                .node
                .lock()
                .map_err(|_| vm.new_runtime_error("coin: the field is busy"))?;
            let parsed = parse_elem(zelf.field, &value, vm)?;
            write(&mut node, zelf.field, &[parsed], vm)
        }

        /// `field.set1Value(index, value)` — set one element, growing to reach it.
        #[pymethod]
        fn set1Value(
            zelf: &Py<Self>,
            index: usize,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            let mut node = zelf
                .node
                .lock()
                .map_err(|_| vm.new_runtime_error("coin: the field is busy"))?;
            // A node list takes a node, not a number.
            if core::nodes_mut(&mut node, zelf.field).is_some() {
                let child = node_arg(&value, vm)?;
                let Some(children) = core::nodes_mut(&mut node, zelf.field) else {
                    unreachable!("checked above")
                };
                if children.len() <= index {
                    children.resize(index + 1, Node::default());
                }
                children[index] = child;
                return Ok(());
            }
            let parsed = parse_elem(zelf.field, &value, vm)?;
            let mut buffer = read(&node, zelf.field, vm)?;
            grow(&mut buffer, zelf.field, index + 1);
            buffer[index] = parsed;
            write(&mut node, zelf.field, &buffer, vm)
        }

        /// `field.append(value)`.
        #[pymethod]
        fn append(zelf: &Py<Self>, value: PyObjectRef, vm: &VirtualMachine) -> PyResult<()> {
            let mut node = zelf
                .node
                .lock()
                .map_err(|_| vm.new_runtime_error("coin: the field is busy"))?;
            if core::nodes_mut(&mut node, zelf.field).is_some() {
                let child = node_arg(&value, vm)?;
                if let Some(children) = core::nodes_mut(&mut node, zelf.field) {
                    children.push(child);
                }
                return Ok(());
            }
            let parsed = parse_elem(zelf.field, &value, vm)?;
            let mut buffer = read(&node, zelf.field, vm)?;
            buffer.push(parsed);
            write(&mut node, zelf.field, &buffer, vm)
        }
    }

    impl Representable for PyField {
        fn repr_str(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<String> {
            let count = match zelf.node.lock() {
                Ok(node) => match core::nodes(&node, zelf.field) {
                    Some(children) => Some(children.len()),
                    None => read(&node, zelf.field, vm)
                        .ok()
                        .map(|values| values.len()),
                },
                Err(_) => None,
            };
            Ok(match count {
                Some(count) => format!("{} (len {count})", zelf.field.label()),
                None => format!("{} (detached)", zelf.field.label()),
            })
        }
    }

    // -----------------------------------------------------------------------
    // Scalar and field members of the node classes
    // -----------------------------------------------------------------------

    #[pyclass(with(Constructor))]
    impl PyPickStyle {
        #[extend_class]
        fn extend_class(ctx: &Context, class: &'static Py<PyType>) {
            class.set_str_attr("SHAPE", ctx.new_int(pick_style::SHAPE), ctx);
            class.set_str_attr("BOUNDING_BOX", ctx.new_int(pick_style::BOUNDING_BOX), ctx);
            class.set_str_attr("UNPICKABLE", ctx.new_int(pick_style::UNPICKABLE), ctx);
        }

        #[pygetset]
        fn style(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<i32> {
            zelf.0.with(vm, |node| match node.kind {
                NodeKind::PickStyle { style } => style,
                _ => pick_style::SHAPE,
            })
        }

        #[pygetset(setter)]
        fn set_style(zelf: &Py<Self>, value: i32, vm: &VirtualMachine) -> PyResult<()> {
            zelf.0.with(vm, |node| {
                if let NodeKind::PickStyle { style: slot } = &mut node.kind {
                    *slot = value;
                }
            })
        }
    }

    #[pyclass(with(Constructor))]
    impl PyLightModel {
        #[extend_class]
        fn extend_class(ctx: &Context, class: &'static Py<PyType>) {
            class.set_str_attr("BASE_COLOR", ctx.new_int(light_model::BASE_COLOR), ctx);
            class.set_str_attr("PHONG", ctx.new_int(light_model::PHONG), ctx);
        }

        #[pygetset]
        fn model(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<i32> {
            zelf.0.with(vm, |node| match node.kind {
                NodeKind::LightModel { model } => model,
                _ => light_model::PHONG,
            })
        }

        #[pygetset(setter)]
        fn set_model(zelf: &Py<Self>, value: i32, vm: &VirtualMachine) -> PyResult<()> {
            zelf.0.with(vm, |node| {
                if let NodeKind::LightModel { model: slot } = &mut node.kind {
                    *slot = value;
                }
            })
        }
    }

    #[pyclass(with(Constructor))]
    impl PyMaterialBinding {
        #[extend_class]
        fn extend_class(ctx: &Context, class: &'static Py<PyType>) {
            class.set_str_attr("OVERALL", ctx.new_int(material_binding::OVERALL), ctx);
            class.set_str_attr("PER_PART", ctx.new_int(material_binding::PER_PART), ctx);
            class.set_str_attr("PER_PART_INDEXED", ctx.new_int(material_binding::PER_PART_INDEXED), ctx);
            class.set_str_attr("PER_FACE", ctx.new_int(material_binding::PER_FACE), ctx);
            class.set_str_attr("PER_FACE_INDEXED", ctx.new_int(material_binding::PER_FACE_INDEXED), ctx);
            class.set_str_attr("PER_VERTEX", ctx.new_int(material_binding::PER_VERTEX), ctx);
            class.set_str_attr("PER_VERTEX_INDEXED", ctx.new_int(material_binding::PER_VERTEX_INDEXED), ctx);
        }

        #[pygetset]
        fn value(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<i32> {
            zelf.0.with(vm, |node| match node.kind {
                NodeKind::MaterialBinding { value } => value,
                _ => material_binding::OVERALL,
            })
        }

        #[pygetset(setter)]
        fn set_value(zelf: &Py<Self>, value: i32, vm: &VirtualMachine) -> PyResult<()> {
            zelf.0.with(vm, |node| {
                if let NodeKind::MaterialBinding { value: slot } = &mut node.kind {
                    *slot = value;
                }
            })
        }
    }

    /// `SoCoordinate3.point`.
    #[pyclass(with(Constructor))]
    impl PyCoordinate3 {
        #[pygetset]
        fn point(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            field_py(&zelf.0.inner, FieldId::Point, vm)
        }

        #[pygetset(setter)]
        fn set_point(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            assign(&zelf.0, FieldId::Point, &value, vm)
        }
    }

    /// `SoNormal.vector`.
    #[pyclass(with(Constructor))]
    impl PyNormal {
        #[pygetset]
        fn vector(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            field_py(&zelf.0.inner, FieldId::Vector, vm)
        }

        #[pygetset(setter)]
        fn set_vector(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            assign(&zelf.0, FieldId::Vector, &value, vm)
        }
    }

    /// `SoMaterial.diffuseColor`.
    #[pyclass(with(Constructor))]
    impl PyMaterial {
        #[pygetset]
        fn diffuseColor(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            field_py(&zelf.0.inner, FieldId::DiffuseColor, vm)
        }

        #[pygetset(setter)]
        fn set_diffuseColor(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            assign(&zelf.0, FieldId::DiffuseColor, &value, vm)
        }
    }

    /// `SoFaceSet.numVertices`.
    #[pyclass(with(Constructor))]
    impl PyFaceSet {
        #[pygetset]
        fn numVertices(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            field_py(&zelf.0.inner, FieldId::NumVertices, vm)
        }

        #[pygetset(setter)]
        fn set_numVertices(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            assign(&zelf.0, FieldId::NumVertices, &value, vm)
        }
    }

    /// `SoShaderProgram.shaderObject` — the linked stages.
    #[pyclass(with(Constructor))]
    impl PyShaderProgram {
        #[pygetset]
        fn shaderObject(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            field_py(&zelf.0.inner, FieldId::ShaderObject, vm)
        }

        #[pygetset(setter)]
        fn set_shaderObject(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            assign(&zelf.0, FieldId::ShaderObject, &value, vm)
        }
    }

    /// `SoIndexedFaceSet.coordIndex`/`normalIndex`.
    #[pyclass(with(Constructor))]
    impl PyIndexedFaceSet {
        #[pygetset]
        fn coordIndex(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            field_py(&zelf.0.inner, FieldId::CoordIndex, vm)
        }

        #[pygetset(setter)]
        fn set_coordIndex(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            assign(&zelf.0, FieldId::CoordIndex, &value, vm)
        }

        #[pygetset]
        fn normalIndex(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            field_py(&zelf.0.inner, FieldId::NormalIndex, vm)
        }

        #[pygetset(setter)]
        fn set_normalIndex(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            assign(&zelf.0, FieldId::NormalIndex, &value, vm)
        }
    }

    /// `SoIndexedLineSet.coordIndex`/`normalIndex`.
    #[pyclass(with(Constructor))]
    impl PyIndexedLineSet {
        #[pygetset]
        fn coordIndex(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            field_py(&zelf.0.inner, FieldId::CoordIndex, vm)
        }

        #[pygetset(setter)]
        fn set_coordIndex(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            assign(&zelf.0, FieldId::CoordIndex, &value, vm)
        }

        #[pygetset]
        fn normalIndex(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            field_py(&zelf.0.inner, FieldId::NormalIndex, vm)
        }

        #[pygetset(setter)]
        fn set_normalIndex(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            assign(&zelf.0, FieldId::NormalIndex, &value, vm)
        }
    }

    /// `SoVertexShader`.
    #[pyclass(with(Constructor))]
    impl PyVertexShader {
        #[pygetset]
        fn source(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<String> {
            zelf.0.with(vm, |node| match &node.kind {
                NodeKind::ShaderStage(stage) if stage.kind == ShaderStageKind::Vertex => {
                    stage.source.clone().unwrap_or_default()
                }
                _ => String::new(),
            })
        }

        #[pygetset(setter)]
        fn set_source(zelf: &Py<Self>, value: String, vm: &VirtualMachine) -> PyResult<()> {
            zelf.0.with(vm, |node| {
                if let NodeKind::ShaderStage(stage) = &mut node.kind {
                    if stage.kind == ShaderStageKind::Vertex {
                        stage.source = (!value.is_empty()).then(|| value.clone());
                    }
                }
            })
        }

        #[pygetset]
        fn sourceProgram(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<String> {
            zelf.0.with(vm, |node| match &node.kind {
                NodeKind::ShaderStage(stage) if stage.kind == ShaderStageKind::Vertex => {
                    stage.source_program.clone().unwrap_or_default()
                }
                _ => String::new(),
            })
        }

        #[pygetset(setter)]
        fn set_sourceProgram(
            zelf: &Py<Self>,
            value: String,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            zelf.0.with(vm, |node| {
                if let NodeKind::ShaderStage(stage) = &mut node.kind {
                    if stage.kind == ShaderStageKind::Vertex {
                        stage.source_program = (!value.is_empty()).then(|| value.clone());
                    }
                }
            })
        }

        #[pygetset]
        fn parameter(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            field_py(&zelf.0.inner, FieldId::Parameter, vm)
        }
    }

    /// `SoFragmentShader`.
    #[pyclass(with(Constructor))]
    impl PyFragmentShader {
        #[pygetset]
        fn source(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<String> {
            zelf.0.with(vm, |node| match &node.kind {
                NodeKind::ShaderStage(stage) if stage.kind == ShaderStageKind::Fragment => {
                    stage.source.clone().unwrap_or_default()
                }
                _ => String::new(),
            })
        }

        #[pygetset(setter)]
        fn set_source(zelf: &Py<Self>, value: String, vm: &VirtualMachine) -> PyResult<()> {
            zelf.0.with(vm, |node| {
                if let NodeKind::ShaderStage(stage) = &mut node.kind {
                    if stage.kind == ShaderStageKind::Fragment {
                        stage.source = (!value.is_empty()).then(|| value.clone());
                    }
                }
            })
        }

        #[pygetset]
        fn sourceProgram(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<String> {
            zelf.0.with(vm, |node| match &node.kind {
                NodeKind::ShaderStage(stage) if stage.kind == ShaderStageKind::Fragment => {
                    stage.source_program.clone().unwrap_or_default()
                }
                _ => String::new(),
            })
        }

        #[pygetset(setter)]
        fn set_sourceProgram(
            zelf: &Py<Self>,
            value: String,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            zelf.0.with(vm, |node| {
                if let NodeKind::ShaderStage(stage) = &mut node.kind {
                    if stage.kind == ShaderStageKind::Fragment {
                        stage.source_program = (!value.is_empty()).then(|| value.clone());
                    }
                }
            })
        }

        #[pygetset]
        fn parameter(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            field_py(&zelf.0.inner, FieldId::Parameter, vm)
        }
    }

    /// Replace a field wholesale (`coords.point = [...]`).
    fn assign(
        node: &PyNode,
        field: FieldId,
        value: &PyObjectRef,
        vm: &VirtualMachine,
    ) -> PyResult<()> {
        let parsed = elems(field, value, vm)?;
        node.with(vm, |node| write(node, field, &parsed, vm))?
    }

    /// `SoDepthBuffer`: `test`/`write`/`function` and the compare-function constants.
    #[pyclass(with(Constructor))]
    impl PyDepthBuffer {
        #[extend_class]
        fn extend_class(ctx: &Context, class: &'static Py<PyType>) {
            for (name, value) in [
                ("NEVER", depth_function::NEVER),
                ("LESS", depth_function::LESS),
                ("EQUAL", depth_function::EQUAL),
                ("LEQUAL", depth_function::LEQUAL),
                ("GREATER", depth_function::GREATER),
                ("NOTEQUAL", depth_function::NOTEQUAL),
                ("GEQUAL", depth_function::GEQUAL),
                ("ALWAYS", depth_function::ALWAYS),
            ] {
                class.set_str_attr(name, ctx.new_int(value), ctx);
            }
        }

        #[pygetset]
        fn test(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<bool> {
            zelf.0.with(vm, |node| match node.kind {
                NodeKind::DepthBuffer { test, .. } => test,
                _ => true,
            })
        }

        #[pygetset(setter)]
        fn set_test(zelf: &Py<Self>, value: bool, vm: &VirtualMachine) -> PyResult<()> {
            zelf.0.with(vm, |node| {
                if let NodeKind::DepthBuffer { test, .. } = &mut node.kind {
                    *test = value;
                }
            })
        }

        #[pygetset]
        fn write(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<bool> {
            zelf.0.with(vm, |node| match node.kind {
                NodeKind::DepthBuffer { write, .. } => write,
                _ => true,
            })
        }

        #[pygetset(setter)]
        fn set_write(zelf: &Py<Self>, value: bool, vm: &VirtualMachine) -> PyResult<()> {
            zelf.0.with(vm, |node| {
                if let NodeKind::DepthBuffer { write, .. } = &mut node.kind {
                    *write = value;
                }
            })
        }

        #[pygetset]
        fn function(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<i32> {
            zelf.0.with(vm, |node| match node.kind {
                NodeKind::DepthBuffer { function, .. } => function,
                _ => depth_function::LESS,
            })
        }

        #[pygetset(setter)]
        fn set_function(zelf: &Py<Self>, value: i32, vm: &VirtualMachine) -> PyResult<()> {
            zelf.0.with(vm, |node| {
                if let NodeKind::DepthBuffer { function, .. } = &mut node.kind {
                    *function = value;
                }
            })
        }
    }

    /// `name` is one slot on every parameter shape.
    fn set_parameter_name(parameter: &mut ShaderParameter, value: &str) {
        match parameter {
            ShaderParameter::Float { name, .. }
            | ShaderParameter::Vec3 { name, .. }
            | ShaderParameter::Floats { name, .. } => *name = value.to_string(),
        }
    }

    /// A scalar parameter (`SoShaderParameter1f`): `name` and `value`.
    #[pyclass(with(Constructor))]
    impl PyShaderParameter1f {
        #[pygetset]
        fn name(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<String> {
            zelf.0.with(vm, |node| match &node.kind {
                NodeKind::ShaderParameter(parameter) => parameter.name().to_string(),
                _ => String::new(),
            })
        }

        #[pygetset(setter)]
        fn set_name(zelf: &Py<Self>, value: String, vm: &VirtualMachine) -> PyResult<()> {
            zelf.0.with(vm, |node| {
                if let NodeKind::ShaderParameter(parameter) = &mut node.kind {
                    set_parameter_name(parameter, &value);
                }
            })
        }

        #[pygetset]
        fn value(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<f64> {
            zelf.0.with(vm, |node| match &node.kind {
                NodeKind::ShaderParameter(ShaderParameter::Float { value, .. }) => {
                    f64::from(*value)
                }
                _ => 0.0,
            })
        }

        #[pygetset(setter)]
        fn set_value(zelf: &Py<Self>, value: f64, vm: &VirtualMachine) -> PyResult<()> {
            zelf.0.with(vm, |node| {
                if let NodeKind::ShaderParameter(ShaderParameter::Float {
                    value: slot, ..
                }) = &mut node.kind
                {
                    *slot = value as f32;
                }
            })
        }
    }

    /// A 3-vector parameter (`SoShaderParameter3f`).
    #[pyclass(with(Constructor))]
    impl PyShaderParameter3f {
        #[pygetset]
        fn name(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<String> {
            zelf.0.with(vm, |node| match &node.kind {
                NodeKind::ShaderParameter(parameter) => parameter.name().to_string(),
                _ => String::new(),
            })
        }

        #[pygetset(setter)]
        fn set_name(zelf: &Py<Self>, value: String, vm: &VirtualMachine) -> PyResult<()> {
            zelf.0.with(vm, |node| {
                if let NodeKind::ShaderParameter(parameter) = &mut node.kind {
                    set_parameter_name(parameter, &value);
                }
            })
        }

        #[pygetset]
        fn value(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            zelf.0.with(vm, |node| match &node.kind {
                NodeKind::ShaderParameter(ShaderParameter::Vec3 { value, .. }) => {
                    tuple_py(value, vm)
                }
                _ => vm.ctx.none(),
            })
        }

        #[pygetset(setter)]
        fn set_value(zelf: &Py<Self>, value: PyObjectRef, vm: &VirtualMachine) -> PyResult<()> {
            let vector = numbers3(&value, vm)?;
            zelf.0.with(vm, |node| {
                if let NodeKind::ShaderParameter(ShaderParameter::Vec3 {
                    value: slot, ..
                }) = &mut node.kind
                {
                    *slot = vector;
                }
            })
        }
    }

    /// An array parameter (`SoShaderParameterArray1f`): `name`, and `value` is the
    /// `SoMFFloat` field itself.
    #[pyclass(with(Constructor))]
    impl PyShaderParameterArray1f {
        #[pygetset]
        fn name(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<String> {
            zelf.0.with(vm, |node| match &node.kind {
                NodeKind::ShaderParameter(parameter) => parameter.name().to_string(),
                _ => String::new(),
            })
        }

        #[pygetset(setter)]
        fn set_name(zelf: &Py<Self>, value: String, vm: &VirtualMachine) -> PyResult<()> {
            zelf.0.with(vm, |node| {
                if let NodeKind::ShaderParameter(parameter) = &mut node.kind {
                    set_parameter_name(parameter, &value);
                }
            })
        }

        #[pygetset]
        fn value(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            field_py(&zelf.0.inner, FieldId::ParameterValues, vm)
        }
    }

    // -----------------------------------------------------------------------
    // The pull overlay bridge (the same registry the CPython facade fills)
    // -----------------------------------------------------------------------

    /// `coin.addOverlay(node, role=None)` — extract `node` now and keep it for the
    /// shell.
    ///
    /// `role` is the provider feature the scene belongs to (the facade passes it while
    /// a provider callback registers the tree), which is what lets the viewport replay
    /// the grid on a candidate plane.
    #[pyfunction]
    fn addOverlay(
        node: PyRef<PyNode>,
        role: OptionalArg<PyStrRef>,
        vm: &VirtualMachine,
    ) -> PyResult<()> {
        let root = node.snapshot(vm)?;
        let role = role
            .into_option()
            .map(|role| role.to_str().unwrap_or_default().to_string());
        ferrocad_inventor::overlays::put_with_role(
            Arc::as_ptr(&node.inner) as usize,
            Scene { root },
            role,
        );
        Ok(())
    }

    /// `coin.removeOverlay(node)` — drop the overlay for **this** node.
    #[pyfunction]
    fn removeOverlay(node: PyRef<PyNode>) -> bool {
        ferrocad_inventor::overlays::remove(Arc::as_ptr(&node.inner) as usize)
    }

    /// `coin.clearOverlays()` — drop every registered overlay.
    #[pyfunction]
    fn clearOverlays() -> bool {
        ferrocad_inventor::overlays::clear()
    }

    // -----------------------------------------------------------------------
    // Module constants, and the `RootNode` adapter
    // -----------------------------------------------------------------------

    #[pyattr]
    const SO_SWITCH_NONE: i32 = ferrocad_inventor::SO_SWITCH_NONE;
    #[pyattr]
    const SO_SWITCH_INHERIT: i32 = ferrocad_inventor::SO_SWITCH_INHERIT;
    #[pyattr]
    const SO_SWITCH_ALL: i32 = ferrocad_inventor::SO_SWITCH_ALL;

    /// Wrap a core tree as a Python `coin` node, for `ViewObject.RootNode`.
    ///
    /// The tree is the shared body ([`ferrocad_inventor::view_root`]); this adapter is
    /// just the class choice, as the CPython facade's `to_py` is.
    pub(crate) fn root_py(node: Node, vm: &VirtualMachine) -> PyObjectRef {
        PySeparator(
            PyGroup(PyNode::of(node)),
            AtomicI32::new(RENDER_CULLING_AUTO),
        )
        .into_ref(&vm.ctx)
        .into()
    }
}
