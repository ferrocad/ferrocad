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
//! `docs/inventor-plan.md`).

use ferrocad_inventor::{Node, NodeKind, Scene};
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyModule};

mod fields;
mod events;

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
pub struct SoSeparator;

#[pymethods]
impl SoSeparator {
    #[new]
    fn new() -> PyClassInitializer<Self> {
        group_initializer(NodeKind::Separator).add_subclass(SoSeparator)
    }
}

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

/// Build the `SoBase`→`SoNode` part of the initializer around `kind`.
pub(crate) fn node_initializer(kind: NodeKind) -> PyClassInitializer<SoNode> {
    PyClassInitializer::from(SoBase)
        .add_subclass(SoFieldContainer)
        .add_subclass(SoNode {
            node: Node {
                kind,
                children: Vec::new(),
            },
        })
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
            "SoEventCallback" => Some(Py::new(py, events::SoEventCallback::new())?.into_any()),
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
    "SoEventCallback",
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
        "SoGroup" => derives_from("SoNode", parent),
        "SoCoordinate3" | "SoNormal" | "SoMaterial" | "SoTransform" | "SoIndexedFaceSet"
        | "SoIndexedLineSet" | "SoEventCallback" => derives_from("SoNode", parent),
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
