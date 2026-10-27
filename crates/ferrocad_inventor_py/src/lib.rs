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
//! `docs/viewport/inventor-plan.md`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use ferrocad_inventor::node::{depth_function, light_model, pick_style};
use ferrocad_inventor::{Node, NodeKind, Scene};
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};

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
    fn remove_child(mut slf: PyRefMut<'_, Self>, child: &Bound<'_, SoNode>) -> PyResult<()> {
        let target = child
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
    /// `node.set("field VALUE")` (or `"field=VALUE"`) — set one field from a
    /// string, as Coin's `SoFieldContainer::set` does.
    ///
    /// Supported today: `SoDepthBuffer.test`/`write`/`function`,
    /// `SoLightModel.model` and `SoPickStyle.style`. An unknown field or value
    /// raises `ValueError` naming the field.
    #[pyo3(name = "set")]
    fn set(mut slf: PyRefMut<'_, Self>, spec: &str) -> PyResult<()> {
        let (field, value) = split_field_spec(spec)?;
        apply_field(&mut slf.node.kind, field, value)
    }
}

/// Split `"field value"` or `"field=value"` into its two trimmed halves.
fn split_field_spec(spec: &str) -> PyResult<(&str, &str)> {
    let spec = spec.trim();
    let (name, value) = match spec.split_once('=') {
        Some(pair) => pair,
        None => {
            let index = spec.find(char::is_whitespace).ok_or_else(|| {
                PyValueError::new_err(format!("set: expected \"field value\", got {spec:?}"))
            })?;
            (&spec[..index], &spec[index..])
        }
    };
    let (name, value) = (name.trim(), value.trim());
    if name.is_empty() || value.is_empty() {
        return Err(PyValueError::new_err(format!(
            "set: expected \"field value\", got {spec:?}"
        )));
    }
    Ok((name, value))
}

/// Apply a parsed `set` to the node kind, or name the offending field.
fn apply_field(kind: &mut NodeKind, field: &str, value: &str) -> PyResult<()> {
    match kind {
        NodeKind::DepthBuffer {
            test,
            write,
            function,
        } => match field {
            "test" => *test = parse_bool(field, value)?,
            "write" => *write = parse_bool(field, value)?,
            "function" => *function = parse_depth_function(field, value)?,
            _ => return Err(unknown_field("SoDepthBuffer", field)),
        },
        NodeKind::LightModel { model } => match field {
            "model" => *model = parse_light_model(field, value)?,
            _ => return Err(unknown_field("SoLightModel", field)),
        },
        NodeKind::PickStyle { style } => match field {
            "style" => *style = parse_pick_style(field, value)?,
            _ => return Err(unknown_field("SoPickStyle", field)),
        },
        other => return Err(unknown_field(node_type_name(other), field)),
    }
    Ok(())
}

fn unknown_field(node: &str, field: &str) -> PyErr {
    PyValueError::new_err(format!("{node}.set: unknown field {field:?}"))
}

fn parse_bool(field: &str, value: &str) -> PyResult<bool> {
    match value.to_ascii_uppercase().as_str() {
        "TRUE" | "1" => Ok(true),
        "FALSE" | "0" => Ok(false),
        _ => Err(PyValueError::new_err(format!(
            "{field}: expected TRUE or FALSE, got {value:?}"
        ))),
    }
}

fn parse_pick_style(field: &str, value: &str) -> PyResult<i32> {
    match value.to_ascii_uppercase().as_str() {
        "SHAPE" => Ok(pick_style::SHAPE),
        "BOUNDING_BOX" => Ok(pick_style::BOUNDING_BOX),
        "UNPICKABLE" => Ok(pick_style::UNPICKABLE),
        _ => Err(PyValueError::new_err(format!(
            "{field}: expected SHAPE, BOUNDING_BOX or UNPICKABLE, got {value:?}"
        ))),
    }
}

fn parse_light_model(field: &str, value: &str) -> PyResult<i32> {
    match value.to_ascii_uppercase().as_str() {
        "BASE_COLOR" => Ok(light_model::BASE_COLOR),
        "PHONG" => Ok(light_model::PHONG),
        _ => Err(PyValueError::new_err(format!(
            "{field}: expected BASE_COLOR or PHONG, got {value:?}"
        ))),
    }
}

fn parse_depth_function(field: &str, value: &str) -> PyResult<i32> {
    match value.to_ascii_uppercase().as_str() {
        "NEVER" => Ok(depth_function::NEVER),
        "LESS" => Ok(depth_function::LESS),
        "EQUAL" => Ok(depth_function::EQUAL),
        "LEQUAL" => Ok(depth_function::LEQUAL),
        "GREATER" => Ok(depth_function::GREATER),
        "NOTEQUAL" => Ok(depth_function::NOTEQUAL),
        "GEQUAL" => Ok(depth_function::GEQUAL),
        "ALWAYS" => Ok(depth_function::ALWAYS),
        _ => Err(PyValueError::new_err(format!(
            "{field}: expected NEVER, LESS, EQUAL, LEQUAL, GREATER, NOTEQUAL, GEQUAL \
             or ALWAYS, got {value:?}"
        ))),
    }
}

/// The Coin class name for a node kind (used by `set`'s error messages).
fn node_type_name(kind: &NodeKind) -> &str {
    match kind {
        NodeKind::Separator => "SoSeparator",
        NodeKind::Switch { .. } => "SoSwitch",
        NodeKind::Coordinate3(_) => "SoCoordinate3",
        NodeKind::TextureCoordinate2(_) => "SoTextureCoordinate2",
        NodeKind::Texture2(_) => "SoTexture2",
        NodeKind::Normal(_) => "SoNormal",
        NodeKind::Mesh { .. } => "SoIndexedShape",
        NodeKind::FaceSet { .. } => "SoFaceSet",
        NodeKind::PickStyle { .. } => "SoPickStyle",
        NodeKind::LightModel { .. } => "SoLightModel",
        NodeKind::DepthBuffer { .. } => "SoDepthBuffer",
        NodeKind::Material(_) => "SoMaterial",
        NodeKind::PickId(_) => "SoPickId",
        NodeKind::Transform(_) => "SoTransform",
        NodeKind::EventCallback(_) => "SoEventCallback",
        NodeKind::NaviCube(_) => "SoNaviCube",
        NodeKind::ShaderProgram => "SoShaderProgram",
        NodeKind::ShaderStage(stage) => stage.kind.class_name(),
        NodeKind::ShaderParameter(parameter) => parameter.class_name(),
        NodeKind::Custom(name) => name,
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
    // The pull-based overlay bridge (`docs/workbenches/grid-overlay-plan.md` §3.2, G5). The
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

// ---------------------------------------------------------------------------
// The pull-based overlay bridge (G5)
// ---------------------------------------------------------------------------

/// One registered overlay: its extracted scene plus the Python wrapper it came
/// from.
///
/// The wrapper is kept for **identity only**. Coin's scene graph is live; ours is
/// an owned tree, and `addChild` copies, so an overlay is re-extracted from its
/// wrapper on every refresh rather than mutated in place (`_rebind()` on the
/// Python side). Extraction is the documented copy boundary.
struct OverlayEntry {
    node: Py<PyAny>,
    scene: Scene,
}

fn overlay_entries() -> &'static Mutex<Vec<OverlayEntry>> {
    static OVERLAYS: OnceLock<Mutex<Vec<OverlayEntry>>> = OnceLock::new();
    OVERLAYS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Whether the registered set changed since the last [`overlay_scenes`] call.
static OVERLAYS_DIRTY: AtomicBool = AtomicBool::new(false);

/// `coin.addOverlay(node)` — extract `node` now and keep it for the shell.
///
/// The tree is copied immediately (value semantics), so later mutations on the
/// Python side are invisible until the node is re-registered. Adding a node that
/// is already registered re-extracts it (identity is the Python object).
#[pyfunction]
#[pyo3(name = "addOverlay")]
pub fn add_overlay(node: &Bound<'_, PyAny>) -> PyResult<()> {
    let scene = extract_scene(node)?;
    let handle = node.clone().unbind();
    let mut entries = overlay_entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match entries
        .iter_mut()
        .find(|entry| entry.node.as_ptr() == handle.as_ptr())
    {
        Some(entry) => entry.scene = scene,
        None => entries.push(OverlayEntry {
            node: handle,
            scene,
        }),
    }
    OVERLAYS_DIRTY.store(true, Ordering::Release);
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
    if removed {
        OVERLAYS_DIRTY.store(true, Ordering::Release);
    }
    removed
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
    if cleared {
        OVERLAYS_DIRTY.store(true, Ordering::Release);
    }
    cleared
}

/// Whether the registered overlay set changed since the last [`overlay_scenes`].
///
/// A pure check: the shell calls this cheaply on every refresh and only pays for
/// the clone below when it returns `true`.
pub fn overlays_dirty() -> bool {
    OVERLAYS_DIRTY.load(Ordering::Acquire)
}

/// The registered overlay scenes, in registration order.
///
/// Taking the scenes **clears** the dirty flag: this is the pull half of the
/// bridge. The shell merges the returned roots into the viewport's draw without
/// re-extracting or recompiling anything else.
pub fn overlay_scenes() -> Vec<Scene> {
    let entries = overlay_entries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let scenes: Vec<Scene> = entries.iter().map(|entry| entry.scene.clone()).collect();
    // Clear under the lock: an add between the clone and the store would otherwise
    // lose its dirty notification.
    OVERLAYS_DIRTY.store(false, Ordering::Release);
    scenes
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
            add_overlay(&first).unwrap();
            assert!(overlays_dirty(), "adding marks the set dirty");
            let scenes = overlay_scenes();
            assert_eq!(scenes.len(), 1);
            assert!(!overlays_dirty(), "taking the scenes clears the flag");

            // Re-adding the same object replaces its scene rather than duplicating.
            add_overlay(&first).unwrap();
            assert_eq!(overlay_scenes().len(), 1);

            // A structurally identical but distinct object is a different overlay.
            let second = separator();
            add_overlay(&second).unwrap();
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
