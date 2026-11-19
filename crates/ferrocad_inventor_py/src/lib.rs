//! # FerroCAD pivy-compatible `coin` facade (`ferrocad_inventor_py`)
//!
//! `from pivy import coin` is how 75 upstream Python files build scene graphs, and
//! `coin.SoType.fromName("SoBrepEdgeSet").createInstance()` is how they instantiate
//! FreeCAD's custom node classes. This extension reproduces that surface on top of
//! [`ferrocad_inventor`], so a workbench's scene-building code keeps working.
//!
//! The surface has three parts. The **class spine and `SoType` registry** mirror the
//! Coin hierarchy (`SoBase` → `SoFieldContainer` → `SoNode` → `SoGroup` →
//! `SoSeparator`/`SoSwitch`, with the geometry, transform and event nodes as `SoNode`
//! leaves) so `isinstance()` passes. The **geometry field objects** (`SoCoordinate3`,
//! `SoMaterial`, …) are live views over the node data. The **scene bridge**
//! ([`install`]/[`extract_scene`]) copies a Python-built tree into a
//! [`ferrocad_inventor::Scene`], and [`dispatch_event`] delivers picked input to the
//! `SoEventCallback` nodes. Each class wraps the existing, owned
//! [`ferrocad_inventor`] `Node`; the generational arena is deferred (see
//! `docs/developer/viewport/inventor-plan.md`).

use std::sync::{Mutex, OnceLock};

use ferrocad_inventor::{Node, NodeKind, Scene, ShaderParameter};
use pyo3::exceptions::{PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule, PyType};

mod fields;
mod events;
mod values;

pub use events::dispatch_event;

// ---------------------------------------------------------------------------
// The class spine
// ---------------------------------------------------------------------------

/// Root of the node hierarchy (Coin's `SoBase`).
#[pyclass(subclass)]
pub struct SoBase;

#[pymethods]
impl SoBase {
    #[new]
    fn new() -> Self {
        SoBase
    }

    /// The runtime type of this instance, as an [`SoType`].
    #[pyo3(name = "getTypeId")]
    fn get_type_id(slf: &Bound<'_, Self>) -> PyResult<SoType> {
        Ok(SoType {
            name: slf.get_type().name()?.to_string(),
        })
    }

    /// The class type, as Coin's `getClassTypeId` spells it.
    ///
    /// A class method, so `coin.SoSeparator.getClassTypeId()` works the way a mod's
    /// `isOfType` check expects; it is inherited by every subclass of `SoBase`.
    #[classmethod]
    #[pyo3(name = "getClassTypeId")]
    fn class_type_id(cls: &Bound<'_, PyType>) -> PyResult<SoType> {
        Ok(SoType::named(&cls.name()?.to_string()))
    }
}

/// Container of fields (Coin's `SoFieldContainer`).
#[pyclass(extends = SoBase, subclass)]
pub struct SoFieldContainer;

#[pymethods]
impl SoFieldContainer {
    #[new]
    fn new() -> (Self, SoBase) {
        (SoFieldContainer, SoBase)
    }
}

/// A scene-graph node. Holds the backing [`ferrocad_inventor`] node.
#[pyclass(extends = SoFieldContainer, subclass)]
pub struct SoNode {
    node: Node,
}

/// A group of children (Coin's `SoGroup`).
#[pyclass(extends = SoNode, subclass)]
pub struct SoGroup;

#[pymethods]
impl SoGroup {
    /// `group.addChild(child)` — appends a copy of `child`'s node.
    #[pyo3(name = "addChild")]
    fn add_child(mut slf: PyRefMut<'_, Self>, child: &Bound<'_, SoNode>) -> PyResult<()> {
        let child_node = child.borrow().node.clone();
        slf.as_super().node.children.push(child_node);
        Ok(())
    }

    /// `group.removeChild(node)` — removes the **first** child whose node is
    /// structurally equal to `node`'s node.
    ///
    /// `addChild` copies the child into the group, so Coin's pointer identity is
    /// not available; FerroCAD compares the copied nodes instead. Removing a node
    /// that is not present is a no-op.
    #[pyo3(name = "removeChild")]
    fn remove_child(mut slf: PyRefMut<'_, Self>, child: &Bound<'_, PyAny>) -> PyResult<()> {
        // Coin allows both `removeChild(node)` and `removeChild(index)`.
        if let Ok(index) = child.extract::<usize>() {
            let children = &mut slf.as_super().node.children;
            if index < children.len() {
                children.remove(index);
            }
            return Ok(());
        }
        let target = child
            .downcast::<SoNode>()
            .map_err(|_| PyTypeError::new_err("removeChild expects a node or an index"))?
            .try_borrow()
            .map_err(|_| PyTypeError::new_err("removeChild: the node is already in use"))?
            .node
            .clone();
        if let Some(index) = slf
            .as_super()
            .node
            .children
            .iter()
            .position(|candidate| *candidate == target)
        {
            slf.as_super().node.children.remove(index);
        }
        Ok(())
    }

    /// `group.getNumChildren()`.
    #[pyo3(name = "getNumChildren")]
    fn get_num_children(slf: PyRef<'_, Self>) -> usize {
        slf.as_super().node.children.len()
    }

    /// `group.getChild(index)` — the child at `index`, or `None`.
    ///
    /// The returned node is a detached copy (the same value semantics `addChild` uses
    /// in reverse), so mutating it does not change the group until it is re-inserted.
    #[pyo3(name = "getChild")]
    fn get_child(slf: PyRef<'_, Self>, index: usize, py: Python<'_>) -> PyResult<Option<PyObject>> {
        let Some(node) = slf.as_super().node.children.get(index).cloned() else {
            return Ok(None);
        };
        to_py(py, &node).map(Some)
    }

    /// `group.insertChild(child, index)` — insert a copy at `index`.
    ///
    /// An index past the end appends, matching Coin's clamp.
    #[pyo3(name = "insertChild")]
    fn insert_child(mut slf: PyRefMut<'_, Self>, child: &Bound<'_, SoNode>, index: usize) {
        let child_node = child.borrow().node.clone();
        let children = &mut slf.as_super().node.children;
        let index = index.min(children.len());
        children.insert(index, child_node);
    }

    /// `group.removeAllChildren()`.
    #[pyo3(name = "removeAllChildren")]
    fn remove_all_children(mut slf: PyRefMut<'_, Self>) {
        slf.as_super().node.children.clear();
    }

    /// `group.findChild(node)` — the index of the first structurally equal child, or
    /// `-1`.
    ///
    /// FerroCAD copies on `addChild`, so Coin's pointer identity is unavailable; the
    /// search compares the copied nodes instead.
    #[pyo3(name = "findChild")]
    fn find_child(slf: PyRef<'_, Self>, child: &Bound<'_, SoNode>) -> PyResult<i32> {
        let target = child
            .try_borrow()
            .map_err(|_| PyTypeError::new_err("findChild: the node is already in use"))?
            .node
            .clone();
        Ok(slf
            .as_super()
            .node
            .children
            .iter()
            .position(|candidate| *candidate == target)
            .map_or(-1, |index| index as i32))
    }

    fn __len__(slf: PyRef<'_, Self>) -> usize {
        slf.as_super().node.children.len()
    }
}

/// Groups children and isolates field/coordinate state (Coin's `SoSeparator`).
#[pyclass(extends = SoGroup)]
pub struct SoSeparator {
    /// `renderCulling`, accepted and stored but not applied (FerroCAD draws every
    /// packet, so there is nothing to cull).
    render_culling: i32,
}

#[pymethods]
impl SoSeparator {
    #[new]
    fn new() -> PyClassInitializer<Self> {
        group_initializer(NodeKind::Separator).add_subclass(SoSeparator {
            render_culling: RENDER_CULLING_AUTO,
        })
    }

    // `renderCulling` values. The addon only assigns the constant, so the numeric
    // values are immaterial; Coin's exact values are not verified locally.
    #[classattr]
    const OFF: i32 = RENDER_CULLING_OFF;
    #[classattr]
    const ON: i32 = RENDER_CULLING_ON;
    #[classattr]
    const AUTO: i32 = RENDER_CULLING_AUTO;

    #[getter]
    #[pyo3(name = "renderCulling")]
    fn render_culling(slf: PyRef<'_, Self>) -> i32 {
        slf.render_culling
    }

    #[setter]
    #[pyo3(name = "renderCulling")]
    fn set_render_culling(mut slf: PyRefMut<'_, Self>, value: i32) {
        slf.render_culling = value;
    }
}

const RENDER_CULLING_OFF: i32 = 0;
const RENDER_CULLING_ON: i32 = 1;
const RENDER_CULLING_AUTO: i32 = 2;

/// Draws only the selected child (Coin's `SoSwitch`).
#[pyclass(extends = SoGroup)]
pub struct SoSwitch;

#[pymethods]
impl SoSwitch {
    #[new]
    fn new() -> PyClassInitializer<Self> {
        group_initializer(NodeKind::Switch { which: 0 }).add_subclass(SoSwitch)
    }

    /// `switch.whichChild` — the active child index, or a `SO_SWITCH_*` sentinel.
    #[getter]
    #[pyo3(name = "whichChild")]
    fn which_child(slf: PyRef<'_, Self>) -> i32 {
        match &slf.as_super().as_super().node.kind {
            NodeKind::Switch { which } => *which,
            _ => 0,
        }
    }

    #[setter]
    #[pyo3(name = "whichChild")]
    fn set_which_child(mut slf: PyRefMut<'_, Self>, which: i32) {
        if let NodeKind::Switch { which: current } = &mut slf.as_super().as_super().node.kind {
            *current = which;
        }
    }
}

// ---------------------------------------------------------------------------
// `SoNode.set("field value")` — Coin's `SoFieldContainer::set`
// ---------------------------------------------------------------------------

#[pymethods]
impl SoNode {
    /// `node.isOfType(type)` — whether this node's class is `type` or derives from it.
    ///
    /// This is the classic Coin spelling (`node.isOfType(coin.SoSeparator.getClassTypeId())`),
    /// used by mods that walk a scene graph and pick out shapes.
    #[pyo3(name = "isOfType")]
    fn is_of_type(slf: &Bound<'_, Self>, type_id: &SoType) -> PyResult<bool> {
        let name = slf.get_type().name()?.to_string();
        Ok(derives_from(&name, type_id.name()))
    }

    /// `node.set("field VALUE")` (or `"field=VALUE"`) — set one field from a
    /// string, as Coin's `SoFieldContainer::set` does.
    ///
    /// Supported today: `SoDepthBuffer.test`/`write`/`function`,
    /// `SoLightModel.model` and `SoPickStyle.style`. An unknown field or value
    /// raises `ValueError` naming the field.
    #[pyo3(name = "set")]
    fn set(mut slf: PyRefMut<'_, Self>, spec: &str) -> PyResult<()> {
        ferrocad_inventor::fields::set_from_text(&mut slf.node, spec)
            .map_err(|error| PyValueError::new_err(error.to_string()))
    }
}

/// Build the `SoBase`→`SoNode` part of the initializer around `kind`.
pub(crate) fn node_initializer(kind: NodeKind) -> PyClassInitializer<SoNode> {
    node_snapshot_initializer(Node {
        kind,
        children: Vec::new(),
    })
}

/// Build the `SoBase`→`SoNode` part of the initializer around a copied node,
/// children included (the detached snapshots an `SoMFNode` read returns).
pub(crate) fn node_snapshot_initializer(node: Node) -> PyClassInitializer<SoNode> {
    PyClassInitializer::from(SoBase)
        .add_subclass(SoFieldContainer)
        .add_subclass(SoNode { node })
}

/// Build the `SoBase`→`SoGroup` chain around `kind`.
fn group_initializer(kind: NodeKind) -> PyClassInitializer<SoGroup> {
    node_initializer(kind).add_subclass(SoGroup)
}

// ---------------------------------------------------------------------------
// The type registry (`SoType`)
// ---------------------------------------------------------------------------

/// A runtime type handle (Coin's `SoType`).
#[pyclass]
pub struct SoType {
    name: String,
}

impl SoType {
    /// A handle for `name` (used by the event classes' `getClassTypeId`).
    pub(crate) fn named(name: &str) -> SoType {
        SoType {
            name: name.to_string(),
        }
    }

    /// The type name.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

#[pymethods]
impl SoType {
    /// `SoType.fromName("SoSeparator")` — `None` for an unknown name.
    #[staticmethod]
    #[pyo3(name = "fromName")]
    fn from_name(name: &str) -> Option<SoType> {
        KNOWN_TYPES
            .contains(&name)
            .then(|| SoType { name: name.to_string() })
    }

    /// The registered type name.
    #[pyo3(name = "getName")]
    fn get_name(&self) -> &str {
        &self.name
    }

    /// `type.createInstance()` — a new node of this type, or `None` if the type is
    /// abstract (has no constructor we expose).
    #[pyo3(name = "createInstance")]
    fn create_instance(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        let instance: Option<PyObject> = match self.name.as_str() {
            "SoSeparator" => Some(Py::new(py, SoSeparator::new())?.into_any()),
            "SoSwitch" => Some(Py::new(py, SoSwitch::new())?.into_any()),
            "SoCoordinate3" => Some(Py::new(py, fields::SoCoordinate3::new())?.into_any()),
            "SoNormal" => Some(Py::new(py, fields::SoNormal::new())?.into_any()),
            "SoMaterial" => Some(Py::new(py, fields::SoMaterial::new())?.into_any()),
            "SoTransform" => Some(Py::new(py, fields::SoTransform::new())?.into_any()),
            "SoIndexedFaceSet" => Some(Py::new(py, fields::SoIndexedFaceSet::new())?.into_any()),
            "SoIndexedLineSet" => Some(Py::new(py, fields::SoIndexedLineSet::new())?.into_any()),
            "SoFaceSet" => Some(Py::new(py, fields::SoFaceSet::new())?.into_any()),
            "SoPickStyle" => Some(Py::new(py, fields::SoPickStyle::new())?.into_any()),
            "SoLightModel" => Some(Py::new(py, fields::SoLightModel::new())?.into_any()),
            "SoDepthBuffer" => Some(Py::new(py, fields::SoDepthBuffer::new())?.into_any()),
            "SoMaterialBinding" => {
                Some(Py::new(py, fields::SoMaterialBinding::new())?.into_any())
            }
            "SoShaderProgram" => Some(Py::new(py, fields::SoShaderProgram::new())?.into_any()),
            "SoVertexShader" => Some(Py::new(py, fields::SoVertexShader::new())?.into_any()),
            "SoFragmentShader" => Some(Py::new(py, fields::SoFragmentShader::new())?.into_any()),
            "SoShaderParameter1f" => {
                Some(Py::new(py, fields::SoShaderParameter1f::new())?.into_any())
            }
            "SoShaderParameter3f" => {
                Some(Py::new(py, fields::SoShaderParameter3f::new())?.into_any())
            }
            "SoShaderParameterArray1f" => {
                Some(Py::new(py, fields::SoShaderParameterArray1f::new())?.into_any())
            }
            "SoEventCallback" => Some(Py::new(py, events::SoEventCallback::new())?.into_any()),
            "SoPerspectiveCamera" => Some(
                Py::new(py, values::SoCamera::initializer("SoPerspectiveCamera")
                    .add_subclass(values::SoPerspectiveCamera))?
                .into_any(),
            ),
            "SoOrthographicCamera" => Some(
                Py::new(py, values::SoCamera::initializer("SoOrthographicCamera")
                    .add_subclass(values::SoOrthographicCamera))?
                .into_any(),
            ),
            _ => None,
        };
        Ok(instance)
    }

    /// Whether this type is `other` or derives from it.
    #[pyo3(name = "isDerivedFrom")]
    fn is_derived_from(&self, other: &SoType) -> bool {
        derives_from(&self.name, &other.name)
    }

    fn __repr__(&self) -> String {
        format!("SoType({:?})", self.name)
    }
}

/// The type names the registry knows.
const KNOWN_TYPES: &[&str] = &[
    "SoBase",
    "SoFieldContainer",
    "SoNode",
    "SoGroup",
    "SoSeparator",
    "SoSwitch",
    "SoCoordinate3",
    "SoNormal",
    "SoMaterial",
    "SoTransform",
    "SoIndexedFaceSet",
    "SoIndexedLineSet",
    "SoFaceSet",
    "SoPickStyle",
    "SoLightModel",
    "SoDepthBuffer",
    "SoMaterialBinding",
    "SoShaderProgram",
    "SoVertexShader",
    "SoFragmentShader",
    "SoShaderParameter1f",
    "SoShaderParameter3f",
    "SoShaderParameterArray1f",
    "SoEventCallback",
    "SoCamera",
    "SoPerspectiveCamera",
    "SoOrthographicCamera",
    "SoEvent",
    "SoButtonEvent",
    "SoMouseButtonEvent",
    "SoLocation2Event",
];

/// Whether `child` is `parent`, or a subtype of it.
pub(crate) fn derives_from(child: &str, parent: &str) -> bool {
    if child == parent {
        return true;
    }
    match child {
        "SoSeparator" | "SoSwitch" => derives_from("SoGroup", parent),
        "SoPerspectiveCamera" | "SoOrthographicCamera" => derives_from("SoCamera", parent),
        "SoCamera" => derives_from("SoNode", parent),
        "SoGroup" => derives_from("SoNode", parent),
        "SoCoordinate3"
        | "SoNormal"
        | "SoMaterial"
        | "SoTransform"
        | "SoIndexedFaceSet"
        | "SoIndexedLineSet"
        | "SoFaceSet"
        | "SoPickStyle"
        | "SoLightModel"
        | "SoDepthBuffer"
        | "SoShaderProgram"
        | "SoVertexShader"
        | "SoFragmentShader"
        | "SoShaderParameter1f"
        | "SoShaderParameter3f"
        | "SoShaderParameterArray1f"
        | "SoEventCallback" => derives_from("SoNode", parent),
        "SoMouseButtonEvent" => derives_from("SoButtonEvent", parent),
        "SoButtonEvent" | "SoLocation2Event" => derives_from("SoEvent", parent),
        "SoNode" => derives_from("SoFieldContainer", parent),
        "SoFieldContainer" => derives_from("SoBase", parent),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// The module
// ---------------------------------------------------------------------------

/// The `coin` extension module (the surface of `pivy.coin`).
#[pymodule]
pub fn coin(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    // `SoSwitch.whichChild` sentinels (Coin exposes these as module constants).
    m.add("SO_SWITCH_NONE", ferrocad_inventor::SO_SWITCH_NONE)?;
    m.add("SO_SWITCH_INHERIT", ferrocad_inventor::SO_SWITCH_INHERIT)?;
    m.add("SO_SWITCH_ALL", ferrocad_inventor::SO_SWITCH_ALL)?;
    m.add_class::<SoBase>()?;
    m.add_class::<SoFieldContainer>()?;
    m.add_class::<SoNode>()?;
    m.add_class::<SoGroup>()?;
    m.add_class::<SoSeparator>()?;
    m.add_class::<SoSwitch>()?;
    m.add_class::<SoType>()?;
    fields::register(m)?;
    events::register(m)?;
    values::register(m)?;
    // The pull-based overlay bridge (`docs/developer/workbenches/grid-overlay-plan.md` §3.2, G5). The
    // shell reads the Rust side; these functions are the Python author surface.
    m.add_function(wrap_pyfunction!(add_overlay, m)?)?;
    m.add_function(wrap_pyfunction!(remove_overlay, m)?)?;
    m.add_function(wrap_pyfunction!(clear_overlays, m)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Registration & the host-facing scene bridge
// ---------------------------------------------------------------------------

/// Register the `coin` module and a `pivy` package in the running interpreter, so
/// a script can `from pivy import coin` — the analogue of `SoFCDB::init()`.
///
/// FreeCAD brings Coin up at GUI startup, then workbenches import it. This is
/// idempotent: a second call returns the already-registered `coin` module.
/// Returns the `coin` module.
pub fn install(py: Python<'_>) -> PyResult<Bound<'_, PyModule>> {
    let modules: Bound<'_, PyDict> = py.import("sys")?.getattr("modules")?.downcast_into()?;
    if let Some(existing) = modules.get_item("pivy.coin")? {
        if let Ok(module) = existing.downcast_into::<PyModule>() {
            return Ok(module);
        }
    }
    let coin = pyo3::wrap_pymodule!(coin)(py).into_bound(py);
    modules.set_item("coin", &coin)?;
    let pivy = PyModule::new(py, "pivy")?;
    pivy.add("coin", &coin)?;
    modules.set_item("pivy", &pivy)?;
    modules.set_item("pivy.coin", &coin)?;
    Ok(coin)
}

/// Extract the scene graph from a Python-built `coin` node.
///
/// `root` is typically a `SoSeparator`; its subtree is copied into a
/// [`ferrocad_inventor::Scene`] the mesh compiler walks, so a workbench that
/// builds the tree with `from pivy import coin` renders through exactly the same
/// `compile` path as the Rust sample scene.
pub fn extract_scene(root: &Bound<'_, PyAny>) -> PyResult<Scene> {
    let node = root
        .downcast::<SoNode>()
        .map_err(|_| PyTypeError::new_err("expected a `coin` scene-graph node"))?
        .borrow();
    Ok(Scene {
        root: node.node.clone(),
    })
}

/// Build the `coin` scene-graph objects for a core [`Node`] tree.
///
/// The body of a view provider's `RootNode` is
/// [`ferrocad_inventor::view_root`]; this is the PyO3 adapter from that neutral tree to
/// the facade's classes. The RustPython host has its own adapter over the same body, so
/// the two runtimes answer the same tree.
pub fn to_py(py: Python<'_>, node: &Node) -> PyResult<PyObject> {
    let name = ferrocad_inventor::class_name(&node.kind).to_string();
    let object = SoType { name: name.clone() }
        .create_instance(py)?
        .ok_or_else(|| PyRuntimeError::new_err(format!("coin: {name} has no constructor")))?;
    let target = object.bind(py);

    match &node.kind {
        NodeKind::Coordinate3(points) => {
            target.setattr("point", points.iter().map(triple).collect::<Vec<_>>())?;
        }
        NodeKind::Normal(normals) => {
            target.setattr("vector", normals.iter().map(triple).collect::<Vec<_>>())?;
        }
        NodeKind::Material(colors) => {
            let colors: Vec<(f64, f64, f64, f64)> = colors
                .iter()
                .map(|c| {
                    (
                        f64::from(c[0]),
                        f64::from(c[1]),
                        f64::from(c[2]),
                        f64::from(c[3]),
                    )
                })
                .collect();
            target.setattr("diffuseColor", colors)?;
        }
        NodeKind::Mesh {
            indices,
            normal_indices,
            ..
        } => {
            target.setattr("coordIndex", indices.clone())?;
            if !normal_indices.is_empty() {
                target.setattr("normalIndex", normal_indices.clone())?;
            }
        }
        NodeKind::FaceSet { num_vertices } => {
            target.setattr("numVertices", num_vertices.clone())?;
        }
        NodeKind::Switch { which } => target.setattr("whichChild", *which)?,
        NodeKind::PickStyle { style } => target.setattr("style", *style)?,
        NodeKind::LightModel { model } => target.setattr("model", *model)?,
        NodeKind::MaterialBinding { value } => target.setattr("value", *value)?,
        NodeKind::DepthBuffer {
            test,
            write,
            function,
        } => {
            target.setattr("test", *test)?;
            target.setattr("write", *write)?;
            target.setattr("function", *function)?;
        }
        NodeKind::ShaderStage(stage) => {
            if let Some(source) = &stage.source {
                target.setattr("source", source)?;
            }
            if let Some(path) = &stage.source_program {
                target.setattr("sourceProgram", path)?;
            }
        }
        NodeKind::ShaderParameter(parameter) => {
            target.setattr("name", parameter.name())?;
            match parameter {
                ShaderParameter::Float { value, .. } => target.setattr("value", *value)?,
                ShaderParameter::Vec3 { value, .. } => target.setattr("value", triple(value))?,
                ShaderParameter::Floats { values, .. } => {
                    let field = target.getattr("value")?;
                    field.call_method1("setValues", (0usize, values.clone()))?;
                }
            }
        }
        _ => {}
    }

    // Children: a program and a stage take a node list (`SoMFNode`); a group takes
    // `addChild`, which copies the child.
    match &node.kind {
        NodeKind::ShaderProgram => {
            let field = target.getattr("shaderObject")?;
            for child in &node.children {
                field.call_method1("append", (to_py(py, child)?,))?;
            }
        }
        NodeKind::ShaderStage(_) => {
            let field = target.getattr("parameter")?;
            for child in &node.children {
                field.call_method1("append", (to_py(py, child)?,))?;
            }
        }
        NodeKind::Separator | NodeKind::Switch { .. } => {
            for child in &node.children {
                target.call_method1("addChild", (to_py(py, child)?,))?;
            }
        }
        _ => {}
    }

    Ok(object)
}

/// A core `[f32; 3]` as the tuple the facade's fields parse.
fn triple(value: &[f32; 3]) -> (f64, f64, f64) {
    (
        f64::from(value[0]),
        f64::from(value[1]),
        f64::from(value[2]),
    )
}

// ---------------------------------------------------------------------------
// The pull-based overlay bridge (G5)
// ---------------------------------------------------------------------------

/// One registered overlay: the Python wrapper it came from.
///
/// The wrapper is kept for **identity only**. Coin's scene graph is live; ours is
/// an owned tree, and `addChild` copies, so an overlay is re-extracted from its
/// wrapper on every refresh rather than mutated in place (`_rebind()` on the
/// Python side). Extraction is the documented copy boundary. The extracted scenes
/// themselves live in [`ferrocad_inventor::overlays`], keyed by this identity, so a
/// platform-neutral host can drain the same registry.
struct OverlayEntry {
    node: Py<PyAny>,
}

fn overlay_entries() -> &'static Mutex<Vec<OverlayEntry>> {
    static OVERLAYS: OnceLock<Mutex<Vec<OverlayEntry>>> = OnceLock::new();
    OVERLAYS.get_or_init(|| Mutex::new(Vec::new()))
}

/// The Python object's identity: the address the pure registry keys on.
fn key(node: &Bound<'_, PyAny>) -> usize {
    node.as_ptr() as usize
}

/// `coin.addOverlay(node, role=None)` — extract `node` now and keep it for the shell.
///
/// The tree is copied immediately (value semantics), so later mutations on the
/// Python side are invisible until the node is re-registered. Adding a node that
/// is already registered re-extracts it (identity is the Python object).
///
/// `role` is the provider feature the scene belongs to, passed by the facade while a
/// provider callback registers the tree; the viewport replays that scene on a
/// candidate plane (the hover bloom). `None` keeps an existing role and registers a
/// scene no plane replay touches.
#[pyfunction]
#[pyo3(name = "addOverlay", signature = (node, role = None))]
pub fn add_overlay(node: &Bound<'_, PyAny>, role: Option<String>) -> PyResult<()> {
    let scene = extract_scene(node)?;
    let key = key(node);
    ferrocad_inventor::overlays::put_with_role(key, scene, role);
    let handle = node.clone().unbind();
    let mut entries = overlay_entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !entries.iter().any(|entry| entry.node.as_ptr() == handle.as_ptr()) {
        entries.push(OverlayEntry { node: handle });
    }
    Ok(())
}

/// `coin.removeOverlay(node)` — drop the overlay for **this** Python object.
///
/// Returns whether anything was removed. Identity, not structural equality, is
/// the key: two identical trees are two overlays.
#[pyfunction]
#[pyo3(name = "removeOverlay")]
pub fn remove_overlay(node: &Bound<'_, PyAny>) -> bool {
    let ptr = node.as_ptr();
    let mut entries = overlay_entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let before = entries.len();
    entries.retain(|entry| entry.node.as_ptr() != ptr);
    let removed = entries.len() != before;
    drop(entries);
    ferrocad_inventor::overlays::remove(key(node)) || removed
}

/// `coin.clearOverlays()` — drop every registered overlay.
#[pyfunction]
#[pyo3(name = "clearOverlays")]
pub fn clear_overlays() -> bool {
    let mut entries = overlay_entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let cleared = !entries.is_empty();
    entries.clear();
    drop(entries);
    ferrocad_inventor::overlays::clear() || cleared
}

/// Whether the registered overlay set changed since the last [`overlay_scenes`].
///
/// A pure check: the shell calls this cheaply on every refresh and only pays for
/// the clone below when it returns `true`.
pub fn overlays_dirty() -> bool {
    ferrocad_inventor::overlays::dirty()
}

/// The registered overlay scenes, in registration order.
///
/// Taking the scenes **clears** the dirty flag: this is the pull half of the
/// bridge. The shell merges the returned roots into the viewport's draw without
/// re-extracting or recompiling anything else.
pub fn overlay_scenes() -> Vec<Scene> {
    ferrocad_inventor::overlays::take_scenes()
}

/// [`overlay_scenes`] with each scene's provider role, for a host that must tell the
/// grid from the rest (the candidate-plane replay).
pub fn overlay_scenes_tagged() -> Vec<ferrocad_inventor::overlays::OverlayScene> {
    ferrocad_inventor::overlays::take_scenes_with_roles()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shader family exists as Coin classes, its fields round-trip, and a tree
    /// built from it extracts into a packet that carries the program (G3).
    #[test]
    fn shader_value_nodes_extract_into_a_program() {
        Python::with_gil(|py| {
            let coin = install(py).unwrap();
            let node = |name: &str| coin.getattr(name).unwrap().call0().unwrap();

            let vertex = node("SoVertexShader");
            vertex.setattr("source", "@fragment fn main() {}").unwrap();
            assert_eq!(
                vertex
                    .getattr("source")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "@fragment fn main() {}"
            );
            vertex
                .setattr("sourceProgram", "/tmp/never-read.wgsl")
                .unwrap();
            assert_eq!(
                vertex
                    .getattr("sourceProgram")
                    .unwrap()
                    .extract::<String>()
                    .unwrap(),
                "/tmp/never-read.wgsl"
            );

            let spacing = node("SoShaderParameter1f");
            spacing.setattr("name", "base_spacing").unwrap();
            spacing.setattr("value", 2.5f32).unwrap();
            assert_eq!(
                spacing.getattr("value").unwrap().extract::<f32>().unwrap(),
                2.5
            );

            let colour = node("SoShaderParameter3f");
            colour.setattr("name", "grid_color").unwrap();
            colour.setattr("value", (1.0f32, 0.5f32, 0.25f32)).unwrap();

            // `SoMFFloat`, with the macro's `setNum`/3-arg `setValues`.
            let shadows = node("SoShaderParameterArray1f");
            shadows.setattr("name", "shadow_data").unwrap();
            let values = shadows.getattr("value").unwrap();
            values.call_method1("setNum", (3usize,)).unwrap();
            assert_eq!(values.len().unwrap(), 3);
            values
                .call_method1("setValues", (0usize, 3usize, vec![1.0f32, 2.0, 3.0]))
                .unwrap();
            assert_eq!(values.len().unwrap(), 3);

            let fragment = node("SoFragmentShader");
            fragment
                .getattr("parameter")
                .unwrap()
                .call_method1(
                    "setValues",
                    (0usize, 3usize, vec![&spacing, &colour, &shadows]),
                )
                .unwrap();
            assert_eq!(fragment.getattr("parameter").unwrap().len().unwrap(), 3);

            let program = node("SoShaderProgram");
            let shader_objects = program.getattr("shaderObject").unwrap();
            shader_objects
                .call_method1("set1Value", (0usize, &vertex))
                .unwrap();
            shader_objects
                .call_method1("set1Value", (1usize, &fragment))
                .unwrap();
            assert_eq!(shader_objects.len().unwrap(), 2);

            let root = node("SoSeparator");
            root.call_method1("addChild", (&program,)).unwrap();
            let coords = node("SoCoordinate3");
            coords
                .getattr("point")
                .unwrap()
                .call_method1(
                    "setValues",
                    (
                        0usize,
                        vec![
                            (0.0f32, 0.0f32, 0.0f32),
                            (1.0f32, 0.0f32, 0.0f32),
                            (0.0f32, 1.0f32, 0.0f32),
                        ],
                    ),
                )
                .unwrap();
            let face_set = node("SoFaceSet");
            face_set
                .getattr("numVertices")
                .unwrap()
                .call_method1("setValues", (0usize, vec![3u32]))
                .unwrap();
            root.call_method1("addChild", (&coords,)).unwrap();
            root.call_method1("addChild", (&face_set,)).unwrap();

            let scene = extract_scene(&root).unwrap();
            let (queue, _) = ferrocad_inventor::compile(&scene);
            assert_eq!(queue.len(), 1, "the program node itself draws nothing");
            let program = queue.packets[0]
                .state
                .program
                .as_ref()
                .expect("the extracted packet carries the program");
            assert_eq!(program.source(), "@fragment fn main() {}");
            assert_eq!(program.parameters().base_spacing, 2.5);
            assert_eq!(program.parameters().color, [1.0, 0.5, 0.25]);
            // `shadow_data` is accepted into the deferred slot, not applied.
            assert_eq!(program.parameters().shadow_count, 0.0);
        });
    }

    /// G5: the pull registry extracts on add, keys removal by Python identity and
    /// reports a dirty flag the shell can poll without cloning.
    #[test]
    fn overlays_register_extract_remove_and_clear_by_identity() {
        Python::with_gil(|py| {
            let coin = install(py).unwrap();
            // The Python surface names are the Coin-extension spellings.
            for name in ["addOverlay", "removeOverlay", "clearOverlays"] {
                assert!(coin.hasattr(name).unwrap(), "coin.{name} is missing");
            }
            let separator = || coin.getattr("SoSeparator").unwrap().call0().unwrap();

            clear_overlays();
            let _ = overlay_scenes();
            assert!(!overlays_dirty());

            let first = separator();
            add_overlay(&first, None).unwrap();
            assert!(overlays_dirty(), "adding marks the set dirty");
            let scenes = overlay_scenes();
            assert_eq!(scenes.len(), 1);
            assert!(!overlays_dirty(), "taking the scenes clears the flag");

            // Re-adding the same object replaces its scene rather than duplicating.
            add_overlay(&first, None).unwrap();
            assert_eq!(overlay_scenes().len(), 1);

            // A structurally identical but distinct object is a different overlay.
            let second = separator();
            add_overlay(&second, None).unwrap();
            assert_eq!(overlay_scenes().len(), 2);
            assert!(!remove_overlay(&separator()), "identity, not equality");
            assert!(remove_overlay(&first));
            assert_eq!(overlay_scenes().len(), 1);

            assert!(clear_overlays());
            assert!(overlays_dirty());
            assert!(overlay_scenes().is_empty());
            assert!(!clear_overlays(), "clearing an empty set is a no-op");
        });
    }
}
