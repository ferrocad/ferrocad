//! The `ferrocad` native module RustPython imports.
//!
//! `python/FreeCAD` (and `python/ferrocad_shell`'s `_fc` calls) use this binding. It is
//! the one piece that cannot be shared with the CPython half: `ferrocad_py` is a PyO3
//! extension, and a `wasm32-unknown-unknown` interpreter has no C ABI to load it with.
//! The *types and rules* are `ferrocad_core`'s; only the binding is per-VM.
//!
//! **The document is writable when the host owns it.** `activeDocument`/`getDocument`/
//! `listDocuments` read the payload `ferrocad_doc_api::payload` renders from the document
//! the host holds, and answer with real `Document`/`DocumentObject` instances. On a host
//! whose document is the process-wide registry (the browser's `live()` composition, and
//! the desktop preview's), `Document.addObject`, `Document.recompute` and a property
//! assignment call `ferrocad_doc_api`'s doorway — the same calls the JS and wasm halves
//! make — so a script's write is the one the model tree and the stage read afterwards.
//! A fixed-payload host (the crate's sample, and a test) refuses them: its document is a
//! projection nobody else can see. Creating, opening and closing a *document* raise on
//! every host: the document half owns those, and the shell reaches them through its own
//! bridge.
//!
//! **Value types.** `Quantity`, `Unit`, `Vector`, `Vector2d`, `Matrix`, `Rotation`,
//! `Placement`, `TypeId`, `BoundBox`, `Material`, `StringID` and `StringHasher` are bound
//! over the same `ferrocad_types`/`ferrocad_core` structures the CPython module uses, so
//! a workplane computation in a browser console means what it means natively.
//!
//! **The view object is the document's, not a copy.** `obj.ViewObject` reads and writes
//! `ferrocad_core::ViewState` on the object in the registry — the state
//! `ferrocad_core::effective_color`/`drawable` give the viewport — so `Visibility`,
//! `ShapeColor`, `Transparency` and `DisplayMode` set from a script are drawn on the next
//! sync. `Doc.Meta`/`Doc.settings` are process state (core does not persist them), as in
//! the CPython binding.
//!
//! **Document observers** are stored, not pumped: this half has no document-event loop,
//! and the shell delivers document events to `FerroCADGui` instead.

pub(crate) use ferrocad::{bound_box_py, module_def, object_named_py, vector_of, vector_py};

use std::cell::RefCell;
use std::collections::BTreeMap;

use ferrocad_core::geometry::{Matrix4, Placement, Rotation, Vector3};
use ferrocad_core::quantity::{Quantity, canonical_name};
use ferrocad_core::unit::Unit;
use rustpython_vm::builtins::{PyDict, PyDictRef, PyStr, PyStrRef, PyType};
use rustpython_vm::function::{FuncArgs, OptionalArg, PySetterValue};
use rustpython_vm::types::{Constructor, SetAttr};
use rustpython_vm::{
    Py, PyObjectRef, PyPayload, PyRef, PyResult, VirtualMachine, pyclass, pymodule,
};

/// One object as the payload describes it.
#[derive(Debug, Clone, serde::Deserialize)]
struct ObjectRow {
    #[serde(default)]
    name: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    type_id: String,
    #[serde(default)]
    properties: Vec<PropertyRow>,
    #[serde(default)]
    driven: Vec<String>,
}

/// One property row.
#[derive(Debug, Clone, serde::Deserialize)]
struct PropertyRow {
    #[serde(default)]
    name: String,
    #[serde(default)]
    type_id: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    value: String,
    #[serde(default)]
    status: Vec<String>,
    #[serde(default)]
    choices: Vec<String>,
}

/// A document as the payload describes it.
#[derive(Debug, Clone, serde::Deserialize)]
struct DocumentRow {
    #[serde(default)]
    name: String,
    #[serde(default)]
    undo_count: i64,
    #[serde(default)]
    redo_count: i64,
    #[serde(default)]
    objects: Vec<ObjectRow>,
}

impl ObjectRow {
    fn property(&self, name: &str) -> Option<&PropertyRow> {
        self.properties.iter().find(|row| row.name == name)
    }
}

/// The active document, as the host renders it, or `None` when nothing is open.
fn active_row() -> Option<DocumentRow> {
    let text = crate::document_json();
    if text.trim() == "null" {
        return None;
    }
    serde_json::from_str(&text).ok()
}

type Observers = RefCell<Vec<PyObjectRef>>;

/// One `ferrocad_doc_api` call: the response's `result`, or the doorway's refusal.
///
/// This is the one road to the document a non-Rust runtime has (the JS half and a wasm
/// bundle use the same protocol), so a Python write and a JS write are the same write.
fn doorway(request: serde_json::Value) -> Result<serde_json::Value, String> {
    use ferrocad_shell_api::DocumentApi as _;

    let answer = ferrocad_doc_api::DocApi::new().call(&request.to_string());
    let answer: serde_json::Value =
        serde_json::from_str(&answer).map_err(|error| format!("document doorway: {error}"))?;
    if answer.get("ok").and_then(|ok| ok.as_bool()) == Some(true) {
        return Ok(answer
            .get("result")
            .cloned()
            .unwrap_or(serde_json::Value::Null));
    }
    Err(answer
        .get("error")
        .and_then(|error| error.as_str())
        .unwrap_or("the document refused the write")
        .to_string())
}

thread_local! {
    /// Document observers a script registered. See the module docs: stored, not pumped.
    static OBSERVERS: Observers = const { RefCell::new(Vec::new()) };

    /// `Document.Meta` for this process.
    ///
    /// The CPython binding keeps it on the `Document` object it owns, and core never
    /// persists it, so a script's annotation styles last for the session. A `Document`
    /// here is a projection rebuilt on every access, hence the document *name* is the
    /// key. Nothing else reads it: the facade's forms are its only consumer.
    static DOCUMENT_META: RefCell<BTreeMap<String, BTreeMap<String, String>>> =
        const { RefCell::new(BTreeMap::new()) };
}

#[pymodule]
mod ferrocad {
    #![allow(non_snake_case, non_upper_case_globals)]

    use super::{
        Constructor, DOCUMENT_META, DocumentRow, FuncArgs, Matrix4, OBSERVERS, ObjectRow,
        OptionalArg, Placement, Py, PyDict, PyDictRef, PyObjectRef, PyPayload, PyRef, PyResult,
        PySetterValue, PyStr, PyStrRef, PyType, Quantity, Rotation, SetAttr, Unit, Vector3,
        VirtualMachine, active_row, canonical_name, doorway, pyclass,
    };
    use std::cell::RefCell;

    #[pyattr]
    const __version__: &str = "0.1.5";

    /// The geometry kernel this build links.
    #[pyattr]
    const _kernel_id: &str = "occt";

    fn attribute_error(
        name: &str,
        vm: &VirtualMachine,
    ) -> rustpython_vm::builtins::PyBaseExceptionRef {
        vm.new_attribute_error(name.to_string())
    }

    /// A Python number as `f64`. `int` is accepted, which `f64::try_from_object` is not.
    fn number(value: &PyObjectRef, vm: &VirtualMachine) -> PyResult<f64> {
        Ok(value.try_float(vm)?.to_f64())
    }

    // --- the document ------------------------------------------------------

    /// One object of the host's document, read from the payload.
    #[pyattr]
    #[pyclass(name = "DocumentObject", module = "ferrocad")]
    #[derive(Debug, PyPayload, Clone)]
    pub struct PyDocumentObject {
        row: ObjectRow,
    }

    #[pyclass(flags(HAS_DICT), with(SetAttr))]
    impl PyDocumentObject {
        #[pygetset]
        fn Name(zelf: &Py<Self>) -> String {
            zelf.row.name.clone()
        }

        #[pygetset]
        fn TypeId(zelf: &Py<Self>) -> String {
            zelf.row.type_id.clone()
        }

        /// The document this object belongs to.
        ///
        /// The facade's view-style store keys on `(document name, object name)`
        /// (`python/FreeCAD/_view_style.py`), and a mod reads `obj.Document.Name`, so the
        /// projection answers it — from the active document, which is the one every row
        /// here came from.
        #[pygetset]
        fn Document(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            match active_row() {
                Some(row)
                    if row
                        .objects
                        .iter()
                        .any(|object| object.name == zelf.row.name) =>
                {
                    Ok(document_py(row, vm))
                }
                _ => Err(vm.new_runtime_error(format!(
                    "{:?} is not in the active document",
                    zelf.row.name
                ))),
            }
        }

        // `Label` has no getter on purpose: it is the object's user-visible name, and a
        // getter would shadow the instance dict that a rename updates (`object_py` seeds
        // it, `reseed` refreshes it). `Name`/`TypeId` never change, so they stay.

        // The view-provider properties below are forwarded to the view object when the
        // object has no property of that name, which is how upstream spells
        // `box.Visibility = False` in a GUI session. A real property wins: a Part feature
        // that stores `DisplayMode` keeps answering from its own row.
        #[pygetset]
        fn Visibility(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            object_row_or_view(&zelf.row, "Visibility", vm)
        }

        #[pygetset]
        fn Transparency(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            object_row_or_view(&zelf.row, "Transparency", vm)
        }

        #[pygetset]
        fn ShapeColor(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            object_row_or_view(&zelf.row, "ShapeColor", vm)
        }

        #[pygetset]
        fn ShapeAppearance(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            object_row_or_view(&zelf.row, "ShapeAppearance", vm)
        }

        #[pygetset]
        fn ShapeMaterial(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            object_row_or_view(&zelf.row, "ShapeMaterial", vm)
        }

        #[pygetset]
        fn DiffuseColor(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            object_row_or_view(&zelf.row, "DiffuseColor", vm)
        }

        #[pygetset]
        fn DisplayMode(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            object_row_or_view(&zelf.row, "DisplayMode", vm)
        }

        #[pygetset]
        fn PropertiesList(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            let names: Vec<PyObjectRef> = zelf
                .row
                .properties
                .iter()
                .map(|row| vm.new_pyobj(row.name.clone()))
                .collect();
            vm.ctx.new_list(names).into()
        }

        /// `[(name, expression)]`; no expressions are stored in this projection.
        #[pygetset]
        fn ExpressionEngine(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            let pairs: Vec<PyObjectRef> = zelf
                .row
                .driven
                .iter()
                .map(|name| {
                    vm.ctx
                        .new_tuple(vec![
                            vm.new_pyobj(name.clone()),
                            vm.new_pyobj(String::new()),
                        ])
                        .into()
                })
                .collect();
            vm.ctx.new_list(pairs).into()
        }

        /// The flag names (`PropertyLength`, …) the payload carries.
        #[pymethod]
        fn getTypeOfProperty(
            zelf: &Py<Self>,
            prop: String,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            let row = zelf
                .row
                .property(&prop)
                .ok_or_else(|| attribute_error(&prop, vm))?;
            let names: Vec<PyObjectRef> = row
                .status
                .iter()
                .map(|name| vm.new_pyobj(name.clone()))
                .collect();
            Ok(vm.ctx.new_list(names).into())
        }

        #[pymethod]
        fn getTypeIdOfProperty(
            zelf: &Py<Self>,
            prop: String,
            vm: &VirtualMachine,
        ) -> PyResult<String> {
            zelf.row
                .property(&prop)
                .map(|row| row.type_id.clone())
                .ok_or_else(|| attribute_error(&prop, vm))
        }

        #[pymethod]
        fn getEnumerationsOfProperty(
            zelf: &Py<Self>,
            prop: String,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            let row = zelf
                .row
                .property(&prop)
                .ok_or_else(|| attribute_error(&prop, vm))?;
            let choices: Vec<PyObjectRef> = row
                .choices
                .iter()
                .map(|choice| vm.new_pyobj(choice.clone()))
                .collect();
            Ok(vm.ctx.new_list(choices).into())
        }

        /// The object's view state (`obj.ViewObject`).
        ///
        /// It is created on first access, from the shape-colour preferences, exactly as
        /// the CPython binding does — the registry object is shared, so the viewport
        /// draws what a script sets.
        #[pygetset]
        fn ViewObject(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            if crate::native_shapes::active_handle().is_err() {
                return Err(vm.new_runtime_error(
                    "pywasm: ViewObject is the registry's; this host's document is a \
                     fixed payload",
                ));
            }
            Ok(PyViewObject::fresh(&zelf.row.name, vm))
        }

        /// A property's display value, as the payload rendered it.
        #[pymethod]
        fn __getattr__(
            zelf: &Py<Self>,
            name: PyStrRef,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            let name = name.to_str().unwrap_or_default();
            if let Some(row) = zelf.row.property(name) {
                return Ok(vm.new_pyobj(row.value.clone()));
            }
            // View-provider properties (`obj.Visibility`, `obj.ShapeColor`, …) are the
            // view object's, as upstream forwards them in a GUI session. A real
            // property of that name wins (`DisplayMode` is one on a Part feature).
            if VIEW_PROPERTIES.contains(&name) {
                let view = PyViewObject::fresh(&zelf.row.name, vm);
                return view.get_attr(vm.ctx.intern_str(name), vm);
            }
            Err(attribute_error(name, vm))
        }
    }

    /// The write path when the host owns the document (B18b item 5).
    ///
    /// The value is rendered as editor text — the form the doorway coerces with
    /// `ferrocad_core::present` — and the instance dict is re-seeded afterwards, because
    /// a recompute can change other rows (a box's `Length` changes its `Shape`, which is
    /// what the script reads next).
    impl SetAttr for PyDocumentObject {
        fn setattro(
            zelf: &Py<Self>,
            name: &Py<PyStr>,
            value: PySetterValue,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            let property = name.to_str().unwrap_or_default().to_string();
            let PySetterValue::Assign(value) = value else {
                // Upstream has no unset for a property row.
                return Err(attribute_error(&property, vm));
            };
            if !crate::document_writes() {
                return Err(read_only("editing a property", vm));
            }
            // A B-Rep is not editor text: `Part.Shape` values are written as the
            // typed property, the way the CPython binding writes them (B18b item 2).
            if property == "Shape" {
                if let Some(shape) = crate::native_shapes::shape_of(&value) {
                    let object = zelf.row.name.clone();
                    crate::native_shapes::set_object_shape(&object, shape)
                        .map_err(|error| vm.new_value_error(error))?;
                    return reseed(zelf, vm);
                }
            }
            // A view property the object does not own belongs to its view object
            // (`box.Visibility = False`), which is how upstream spells it in a GUI
            // session. The row is checked first: a real `DisplayMode` property is a
            // document property.
            if zelf.row.property(&property).is_none()
                && VIEW_PROPERTIES.contains(&property.as_str())
            {
                let view = PyViewObject::fresh(&zelf.row.name, vm);
                return view.set_attr(vm.ctx.intern_str(property.as_str()), value.clone(), vm);
            }
            let text = value.str(vm)?.to_str().unwrap_or_default().to_string();
            let object = zelf.row.name.clone();
            doorway(serde_json::json!({
                "call": "set",
                "object": object,
                "property": property,
                "value": text,
            }))
            .map_err(|error| vm.new_value_error(error))?;
            reseed(zelf, vm)
        }
    }

    // --- view objects -------------------------------------------------------

    /// Properties a document object forwards to its view object when it has no
    /// property of that name (`box.Visibility = False`, as upstream allows in a GUI
    /// session).
    const VIEW_PROPERTIES: &[&str] = &[
        "Visibility",
        "Transparency",
        "ShapeColor",
        "ShapeAppearance",
        "ShapeMaterial",
        "DiffuseColor",
        "DisplayMode",
    ];

    /// Display modes a Part view provider offers (upstream's `ViewProviderPartExt`).
    const DISPLAY_MODES: &[&str] = &["Flat Lines", "Shaded", "Wireframe", "Points"];

    /// A document-object attribute: a real property row first (as editor text, the way
    /// `__getattr__` answers), then the view object's typed value.
    fn object_row_or_view(
        object: &ObjectRow,
        name: &str,
        vm: &VirtualMachine,
    ) -> PyResult<PyObjectRef> {
        if let Some(row) = object.property(name) {
            return Ok(vm.new_pyobj(row.value.clone()));
        }
        if VIEW_PROPERTIES.contains(&name) {
            let view = PyViewObject::fresh(&object.name, vm);
            return view.get_attr(vm.ctx.intern_str(name), vm);
        }
        Err(attribute_error(name, vm))
    }

    /// `App::Origin` and its axes and planes draw opaque whatever the preference says;
    /// upstream's origin view provider overrides the default the same way.
    fn is_origin_family(type_id: &str) -> bool {
        matches!(
            type_id,
            "App::Origin" | "App::Plane" | "App::Line" | "App::Point"
        )
    }

    /// `DefaultShapeColor` / `DefaultShapeTransparency`, read through the facade.
    ///
    /// FreeCAD reads them once, when a view provider is constructed, so the values are
    /// captured into the object's view state. A bare native module (no `FreeCAD` in the
    /// interpreter) gets the classic shape colour, which is what the CPython binding
    /// does without the facade.
    fn view_defaults(vm: &VirtualMachine) -> (i32, ferrocad_core::Color) {
        // FreeCAD's classic shape colour (`0xCCCCE6FF`), as Draft records it.
        const DEFAULT_COLOR: u32 = 0xCCCC_E6FF;
        let mut transparency = 0i32;
        let mut color = ferrocad_core::color_from_packed(DEFAULT_COLOR, true);
        let number = |value: PyObjectRef| value.try_float(vm).ok().map(|number| number.to_f64());
        let Ok(freecad) = vm.import("FreeCAD", 0) else {
            return (transparency, color);
        };
        let Ok(param_get) = freecad.get_attr(vm.ctx.intern_str("ParamGet"), vm) else {
            return (transparency, color);
        };
        let Ok(group) = param_get.call(
            (vm.new_pyobj("User parameter:BaseApp/Preferences/View"),),
            vm,
        ) else {
            return (transparency, color);
        };
        if let Ok(get_int) = group.get_attr(vm.ctx.intern_str("GetInt"), vm) {
            if let Ok(value) = get_int.call(
                (vm.new_pyobj("DefaultShapeTransparency"), vm.new_pyobj(0i64)),
                vm,
            ) {
                if let Some(value) = number(value) {
                    transparency = (value as i64).clamp(0, 100) as i32;
                }
            }
        }
        if let Ok(get_unsigned) = group.get_attr(vm.ctx.intern_str("GetUnsigned"), vm) {
            if let Ok(value) = get_unsigned.call(
                (
                    vm.new_pyobj("DefaultShapeColor"),
                    vm.new_pyobj(DEFAULT_COLOR),
                ),
                vm,
            ) {
                if let Some(value) = number(value) {
                    color = ferrocad_core::color_from_packed(value as u32, true);
                }
            }
        }
        (transparency, color)
    }

    /// One object's view state, created from the preferences on first access.
    ///
    /// The state is `ferrocad_core`'s, on the object in the registry — the same value
    /// the viewport draws and the CPython binding exposes — so a script's write is
    /// visible on the next frame. A host whose document is a fixed payload has no
    /// registry entry and is refused.
    fn view_state_of(object: &str, vm: &VirtualMachine) -> PyResult<ferrocad_core::ViewState> {
        let handle =
            crate::native_shapes::active_handle().map_err(|error| vm.new_runtime_error(error))?;
        let type_id = {
            let document = handle.lock().map_err(|error| {
                vm.new_runtime_error(format!("the document is poisoned: {error}"))
            })?;
            let id = document
                .get_by_name(object)
                .ok_or_else(|| vm.new_attribute_error(format!("no object {object:?}")))?;
            let entry = document
                .object(id)
                .ok_or_else(|| vm.new_attribute_error(format!("no object {object:?}")))?;
            match &entry.view {
                Some(state) => return Ok(state.clone()),
                None => entry.type_id.clone(),
            }
        };
        // The preferences are read without the document locked: `ParamGet` is Python.
        let (mut transparency, color) = view_defaults(vm);
        if is_origin_family(&type_id) {
            transparency = 0;
        }
        let state = ferrocad_core::ViewState::new(transparency, color);
        let mut document = handle
            .lock()
            .map_err(|error| vm.new_runtime_error(format!("the document is poisoned: {error}")))?;
        if let Some(entry) = document
            .get_by_name(object)
            .and_then(|id| document.object_mut(id))
        {
            if entry.view.is_none() {
                entry.view = Some(state.clone());
            }
        }
        Ok(state)
    }

    /// Apply a change to one object's view state.
    fn update_view_state(
        object: &str,
        vm: &VirtualMachine,
        change: impl FnOnce(&mut ferrocad_core::ViewState),
    ) -> PyResult<()> {
        // Materialising first is also what refuses an object this host does not own.
        let _ = view_state_of(object, vm)?;
        let handle =
            crate::native_shapes::active_handle().map_err(|error| vm.new_runtime_error(error))?;
        let mut document = handle
            .lock()
            .map_err(|error| vm.new_runtime_error(format!("the document is poisoned: {error}")))?;
        let id = document
            .get_by_name(object)
            .ok_or_else(|| vm.new_attribute_error(format!("no object {object:?}")))?;
        match document
            .object_mut(id)
            .and_then(|entry| entry.view.as_mut())
        {
            Some(state) => {
                change(state);
                Ok(())
            }
            None => Err(vm.new_runtime_error(format!("{object} has no view state"))),
        }
    }

    /// A colour argument: an `(r, g, b[, a])` sequence, a packed `0xRRGGBBAA` integer,
    /// or a `Material` (whose opacity is its own field, not the colour's alpha).
    fn view_color(value: &PyObjectRef, vm: &VirtualMachine) -> PyResult<ferrocad_core::Color> {
        if let Some(material) = value.downcast_ref::<PyMaterial>() {
            let material = material.inner.borrow();
            let mut color = material.diffuse;
            color[3] = 1.0 - material.transparency;
            return Ok(color);
        }
        if let Ok(channels) = value.clone().try_into_value::<Vec<f64>>(vm) {
            return match channels.as_slice() {
                [r, g, b] => Ok([*r, *g, *b, 1.0]),
                [r, g, b, a] => Ok([*r, *g, *b, *a]),
                _ => Err(vm.new_value_error("a colour takes 3 or 4 numbers (r, g, b[, a])")),
            };
        }
        if let Ok(packed) = value.clone().try_into_value::<u32>(vm) {
            return Ok(ferrocad_core::color_from_packed(packed, false));
        }
        Err(vm.new_type_error("expected an (r, g, b[, a]) colour or a Material"))
    }

    /// A material argument: a `Material`, or a colour (the other fields default).
    fn view_material(
        value: &PyObjectRef,
        vm: &VirtualMachine,
    ) -> PyResult<ferrocad_core::Material> {
        if let Some(material) = value.downcast_ref::<PyMaterial>() {
            return Ok(material.inner.borrow().clone());
        }
        Ok(ferrocad_core::Material::from_color(view_color(value, vm)?))
    }

    /// A `ShapeAppearance` argument: a list of materials or colours, or a single one.
    fn view_materials(
        value: &PyObjectRef,
        vm: &VirtualMachine,
    ) -> PyResult<Vec<ferrocad_core::Material>> {
        if value.downcast_ref::<PyMaterial>().is_some() {
            return Ok(vec![view_material(value, vm)?]);
        }
        let items: Vec<PyObjectRef> = value
            .clone()
            .try_into_value::<Vec<PyObjectRef>>(vm)
            .map_err(|_| vm.new_type_error("expected a list of materials or colours"))?;
        items.iter().map(|item| view_material(item, vm)).collect()
    }

    /// A `DiffuseColor` argument: a list of colours or materials.
    fn view_colors(
        value: &PyObjectRef,
        vm: &VirtualMachine,
    ) -> PyResult<Vec<ferrocad_core::Color>> {
        if value.downcast_ref::<PyMaterial>().is_some() {
            return Ok(vec![view_color(value, vm)?]);
        }
        let items: Vec<PyObjectRef> = value
            .clone()
            .try_into_value::<Vec<PyObjectRef>>(vm)
            .map_err(|_| vm.new_type_error("expected a list of colours"))?;
        items.iter().map(|item| view_color(item, vm)).collect()
    }

    /// A detached `Material` instance (the same class the facade re-exports).
    fn material_py(material: ferrocad_core::Material, vm: &VirtualMachine) -> PyObjectRef {
        PyMaterial {
            inner: RefCell::new(material),
        }
        .into_ref(&vm.ctx)
        .into()
    }

    /// The view object of one document object (`obj.ViewObject`).
    ///
    /// Upstream creates a `Gui::ViewProvider` next to every object only in a GUI
    /// session; FerroCAD keeps the same small model in every session, in
    /// `ferrocad_core::ViewState` on the object itself. This is that state's Python
    /// face for RustPython, so a pywasm script and a CPython one change one value and
    /// the viewport draws it.
    ///
    /// Two members the CPython binding has are absent here on purpose: `RootNode` needs
    /// the Coin bindings (`pivy`), which the CPython binding also refuses without, and
    /// `Proxy` is the `viewProxy=` argument the native `addObject` does not take.
    #[pyattr]
    #[pyclass(name = "ViewObject", module = "ferrocad")]
    #[derive(Debug, PyPayload)]
    pub struct PyViewObject {
        /// The document object, by name — the identity every other road uses.
        object: String,
    }

    impl PyViewObject {
        fn fresh(object: &str, vm: &VirtualMachine) -> PyObjectRef {
            PyViewObject {
                object: object.to_string(),
            }
            .into_ref(&vm.ctx)
            .into()
        }

        /// Per-face colours: an explicit `DiffuseColor`, else the shape colour repeated
        /// once per face (empty when the object has no shape).
        fn diffuse_colors(&self, vm: &VirtualMachine) -> PyResult<Vec<ferrocad_core::Color>> {
            let state = view_state_of(&self.object, vm)?;
            if let Some(colors) = &state.diffuse_color {
                return Ok(colors.clone());
            }
            let color = state.effective_color();
            Ok(
                match crate::native_shapes::object_face_count(&self.object) {
                    Some(count) => vec![color; count],
                    None => Vec::new(),
                },
            )
        }
    }

    #[pyclass]
    impl PyViewObject {
        /// The document object this view object belongs to.
        #[pygetset]
        fn Object(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            object_named(&zelf.object, vm)
                .ok_or_else(|| vm.new_attribute_error(format!("no object {:?}", zelf.object)))
        }

        #[pygetset]
        fn Visibility(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<bool> {
            Ok(view_state_of(&zelf.object, vm)?.visibility)
        }

        #[pygetset(setter)]
        fn set_Visibility(zelf: &Py<Self>, value: bool, vm: &VirtualMachine) -> PyResult<()> {
            update_view_state(&zelf.object, vm, |state| state.visibility = value)
        }

        /// `Transparency` in percent (`0` opaque … `100` invisible).
        #[pygetset]
        fn Transparency(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<i64> {
            Ok(view_state_of(&zelf.object, vm)?.transparency as i64)
        }

        #[pygetset(setter)]
        fn set_Transparency(zelf: &Py<Self>, value: i64, vm: &VirtualMachine) -> PyResult<()> {
            update_view_state(&zelf.object, vm, |state| {
                state.transparency = value.clamp(0, 100) as i32
            })
        }

        /// The colour the shape is painted with: an explicit `ShapeColor`, else the
        /// preference-derived default the view state was created with.
        #[pygetset]
        fn ShapeColor(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<(f64, f64, f64, f64)> {
            Ok(tuple(view_state_of(&zelf.object, vm)?.effective_color()))
        }

        #[pygetset(setter)]
        fn set_ShapeColor(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            let color = view_color(&value, vm)?;
            update_view_state(&zelf.object, vm, |state| state.set_shape_color(color))
        }

        /// `ShapeAppearance`: the shape's material list.
        ///
        /// The entries are copies. CPython hands back handles that write through; here
        /// the list is read and the whole list is what a script assigns.
        #[pygetset]
        fn ShapeAppearance(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let items: Vec<PyObjectRef> = view_state_of(&zelf.object, vm)?
                .appearance()
                .into_iter()
                .map(|material| material_py(material, vm))
                .collect();
            Ok(vm.ctx.new_list(items).into())
        }

        #[pygetset(setter)]
        fn set_ShapeAppearance(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            let materials = view_materials(&value, vm)?;
            update_view_state(&zelf.object, vm, |state| state.set_appearance(materials))
        }

        #[pygetset]
        fn ShapeMaterial(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            Ok(material_py(view_state_of(&zelf.object, vm)?.material(), vm))
        }

        #[pygetset(setter)]
        fn set_ShapeMaterial(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            let material = view_material(&value, vm)?;
            update_view_state(&zelf.object, vm, |state| state.material = Some(material))
        }

        /// One RGBA colour per face, or the shape's whole colour repeated per face.
        #[pygetset]
        fn DiffuseColor(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let items: Vec<PyObjectRef> = zelf
                .diffuse_colors(vm)?
                .into_iter()
                .map(|color| {
                    let channels: Vec<PyObjectRef> =
                        color.into_iter().map(|value| vm.new_pyobj(value)).collect();
                    vm.ctx.new_tuple(channels).into()
                })
                .collect();
            Ok(vm.ctx.new_list(items).into())
        }

        #[pygetset(setter)]
        fn set_DiffuseColor(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            let colors = view_colors(&value, vm)?;
            update_view_state(&zelf.object, vm, |state| state.diffuse_color = Some(colors))
        }

        #[pygetset]
        fn DisplayMode(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<String> {
            Ok(view_state_of(&zelf.object, vm)?.display_mode)
        }

        #[pygetset(setter)]
        fn set_DisplayMode(zelf: &Py<Self>, value: String, vm: &VirtualMachine) -> PyResult<()> {
            if !DISPLAY_MODES.contains(&value.as_str()) {
                return Err(vm.new_value_error(format!("'{value}' is not part of the enumeration")));
            }
            update_view_state(&zelf.object, vm, |state| state.display_mode = value)
        }

        /// The `DisplayMode` choices, as the property pane asks for them.
        #[pymethod]
        fn getEnumerationsOfProperty(
            _zelf: &Py<Self>,
            name: String,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            if name != "DisplayMode" {
                return Err(attribute_error(&name, vm));
            }
            let items: Vec<PyObjectRef> = DISPLAY_MODES
                .iter()
                .map(|mode| vm.new_pyobj(*mode))
                .collect();
            Ok(vm.ctx.new_list(items).into())
        }

        #[pymethod]
        fn __str__(zelf: &Py<Self>) -> String {
            format!("<ViewObject of {}>", zelf.object)
        }
    }

    /// The host's document: name, objects and the undo history depth.
    #[pyattr]
    #[pyclass(name = "Document", module = "ferrocad")]
    #[derive(Debug, PyPayload, Clone)]
    pub struct PyDocument {
        row: DocumentRow,
    }

    #[pyclass]
    impl PyDocument {
        #[pygetset]
        fn Name(zelf: &Py<Self>) -> String {
            zelf.row.name.clone()
        }

        #[pygetset]
        fn Label(zelf: &Py<Self>) -> String {
            zelf.row.name.clone()
        }

        #[pygetset]
        fn Objects(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let mut objects = Vec::with_capacity(zelf.row.objects.len());
            for row in zelf.row.objects.iter().cloned() {
                objects.push(object_py(row, vm)?);
            }
            Ok(vm.ctx.new_list(objects).into())
        }

        #[pygetset]
        fn UndoCount(zelf: &Py<Self>) -> i64 {
            zelf.row.undo_count
        }

        #[pygetset]
        fn RedoCount(zelf: &Py<Self>) -> i64 {
            zelf.row.redo_count
        }

        #[pymethod]
        fn getObject(zelf: &Py<Self>, name: String, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            match zelf
                .row
                .objects
                .iter()
                .find(|row| row.name == name)
                .cloned()
            {
                Some(row) => object_py(row, vm),
                None => Ok(vm.ctx.none()),
            }
        }

        /// Create an object through the document doorway (B18b item 5).
        ///
        /// The name is optional, as upstream (`addObject("Part::Box")` gets FreeCAD's
        /// default); the doorway also applies a `properties` argument, which this binding
        /// does not take yet — a script sets them one at a time, exactly as the property
        /// pane does.
        #[pymethod]
        fn addObject(
            _zelf: &Py<Self>,
            r#type: PyStrRef,
            name: OptionalArg<PyStrRef>,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            if !crate::document_writes() {
                return Err(read_only("adding an object", vm));
            }
            let type_id = r#type.to_str().unwrap_or_default().to_string();
            let name = name
                .into_option()
                .map(|name| name.to_str().unwrap_or_default().to_string())
                .unwrap_or_default();
            let answer = doorway(serde_json::json!({
                "call": "create",
                "type": type_id,
                "name": name,
            }))
            .map_err(|error| vm.new_value_error(error))?;
            let created = answer
                .get("name")
                .and_then(|name| name.as_str())
                .unwrap_or_default()
                .to_string();
            object_named(&created, vm).ok_or_else(|| {
                vm.new_runtime_error(format!(
                    "{created} was created but the document does not list it"
                ))
            })
        }

        /// Recompute through the doorway, answering how many objects executed.
        ///
        /// A read-only host has nothing to recompute (its payload is a projection), so
        /// this is `0` rather than a refusal: scripts call it unconditionally.
        #[pymethod]
        fn recompute(_zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<usize> {
            if !crate::document_writes() {
                return Ok(0);
            }
            let answer = doorway(serde_json::json!({ "call": "recompute" }))
                .map_err(|error| vm.new_value_error(error))?;
            Ok(answer
                .get("recomputed")
                .and_then(|names| names.as_array())
                .map(Vec::len)
                .unwrap_or(0))
        }

        // Accepted, and deliberately no-ops: every doorway write is its own committed
        // transaction, so an outer transaction cannot group or roll back several of them.
        // The facade opens one around an edit (`ferrocad_shell.set_property`), and the
        // doorway's per-write transaction is the undo step it gets.
        #[pymethod]
        fn openTransaction(_zelf: &Py<Self>, _name: OptionalArg<PyStrRef>) {}

        #[pymethod]
        fn commitTransaction(_zelf: &Py<Self>) {}

        #[pymethod]
        fn abortTransaction(_zelf: &Py<Self>) {}

        /// `Doc.Meta`: the session's key/value record.
        ///
        /// Core does not persist it and the CPython binding keeps it on the `Document`
        /// object it owns, so this is process state too; the facade's annotation-style
        /// form is its reader (`python/FerroCADGui/forms.py`).
        #[pygetset]
        fn Meta(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            let entries = DOCUMENT_META.with(|meta| {
                meta.borrow()
                    .get(&zelf.row.name)
                    .cloned()
                    .unwrap_or_default()
            });
            let dict = vm.ctx.new_dict();
            for (key, value) in entries {
                let _ = dict.set_item(&*key, vm.new_pyobj(value), vm);
            }
            dict.into()
        }

        #[pygetset(setter)]
        fn set_Meta(zelf: &Py<Self>, value: PyObjectRef, vm: &VirtualMachine) -> PyResult<()> {
            let dict = value
                .clone()
                .downcast::<PyDict>()
                .map_err(|_| vm.new_type_error("Meta must be a dict"))?;
            let mut entries = std::collections::BTreeMap::new();
            for (key, value) in &dict {
                entries.insert(
                    key.try_to_value::<String>(vm)?,
                    value.try_to_value::<String>(vm)?,
                );
            }
            DOCUMENT_META.with(|meta| meta.borrow_mut().insert(zelf.row.name.clone(), entries));
            Ok(())
        }

        /// A namespaced, typed view over `Meta` (keys `"<namespace>.<key>"`).
        #[pymethod]
        fn settings(
            zelf: &Py<Self>,
            namespace: String,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            if namespace.is_empty() || !namespace.split('.').all(valid_ident) {
                return Err(vm.new_value_error(format!("invalid settings namespace '{namespace}'")));
            }
            Ok(PyDocumentSettings {
                document: zelf.row.name.clone(),
                namespace,
            }
            .into_ref(&vm.ctx)
            .into())
        }
    }

    /// A settings key (`[A-Za-z_][A-Za-z0-9_]*`), as the CPython binding validates it.
    fn valid_ident(name: &str) -> bool {
        let mut chars = name.chars();
        match chars.next() {
            Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
            _ => return false,
        }
        chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    /// `Doc.settings(namespace)`: a typed view over `Document.Meta`.
    #[pyattr]
    #[pyclass(name = "DocumentSettings", module = "ferrocad")]
    #[derive(Debug, PyPayload)]
    pub struct PyDocumentSettings {
        document: String,
        namespace: String,
    }

    impl PyDocumentSettings {
        fn full_key(&self, key: &str) -> String {
            format!("{}.{}", self.namespace, key)
        }

        fn get_raw(&self, key: &str) -> Option<String> {
            let full = self.full_key(key);
            DOCUMENT_META.with(|meta| {
                meta.borrow()
                    .get(&self.document)
                    .and_then(|entries| entries.get(&full).cloned())
            })
        }

        fn set_raw(&self, key: &str, value: String) {
            let full = self.full_key(key);
            DOCUMENT_META.with(|meta| {
                meta.borrow_mut()
                    .entry(self.document.clone())
                    .or_default()
                    .insert(full, value);
            });
        }

        fn remove_raw(&self, key: &str) {
            let full = self.full_key(key);
            DOCUMENT_META.with(|meta| {
                if let Some(entries) = meta.borrow_mut().get_mut(&self.document) {
                    entries.remove(&full);
                }
            });
        }

        fn names(&self) -> Vec<String> {
            let prefix = format!("{}.", self.namespace);
            DOCUMENT_META.with(|meta| {
                meta.borrow()
                    .get(&self.document)
                    .map(|entries| {
                        entries
                            .keys()
                            .filter_map(|key| key.strip_prefix(&prefix))
                            .filter(|rest| !rest.contains('.'))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default()
            })
        }
    }

    #[pyclass]
    impl PyDocumentSettings {
        #[pymethod]
        fn getString(zelf: &Py<Self>, key: String, default: OptionalArg<PyStrRef>) -> String {
            zelf.get_raw(&key).unwrap_or_else(|| {
                default
                    .into_option()
                    .map(|default| default.to_str().unwrap_or_default().to_string())
                    .unwrap_or_default()
            })
        }

        #[pymethod]
        fn getInt(zelf: &Py<Self>, key: String, default: OptionalArg<i64>) -> i64 {
            zelf.get_raw(&key)
                .and_then(|value| value.trim().parse::<i64>().ok())
                .unwrap_or_else(|| default.into_option().unwrap_or(0))
        }

        #[pymethod]
        fn getFloat(zelf: &Py<Self>, key: String, default: OptionalArg<f64>) -> f64 {
            zelf.get_raw(&key)
                .and_then(|value| value.trim().parse::<f64>().ok())
                .unwrap_or_else(|| default.into_option().unwrap_or(0.0))
        }

        #[pymethod]
        fn getBool(zelf: &Py<Self>, key: String, default: OptionalArg<bool>) -> bool {
            zelf.get_raw(&key)
                .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
                    "true" | "1" => Some(true),
                    "false" | "0" => Some(false),
                    _ => None,
                })
                .unwrap_or_else(|| default.into_option().unwrap_or(false))
        }

        #[pymethod]
        fn setString(
            zelf: &Py<Self>,
            key: String,
            value: String,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            if !valid_ident(&key) {
                return Err(vm.new_value_error(format!("invalid setting key '{key}'")));
            }
            zelf.set_raw(&key, value);
            Ok(())
        }

        #[pymethod]
        fn setInt(zelf: &Py<Self>, key: String, value: i64, vm: &VirtualMachine) -> PyResult<()> {
            if !valid_ident(&key) {
                return Err(vm.new_value_error(format!("invalid setting key '{key}'")));
            }
            zelf.set_raw(&key, value.to_string());
            Ok(())
        }

        #[pymethod]
        fn setFloat(zelf: &Py<Self>, key: String, value: f64, vm: &VirtualMachine) -> PyResult<()> {
            if !valid_ident(&key) {
                return Err(vm.new_value_error(format!("invalid setting key '{key}'")));
            }
            zelf.set_raw(&key, format!("{value}"));
            Ok(())
        }

        #[pymethod]
        fn setBool(zelf: &Py<Self>, key: String, value: bool, vm: &VirtualMachine) -> PyResult<()> {
            if !valid_ident(&key) {
                return Err(vm.new_value_error(format!("invalid setting key '{key}'")));
            }
            zelf.set_raw(&key, if value { "true".into() } else { "false".into() });
            Ok(())
        }

        #[pymethod]
        fn remove(zelf: &Py<Self>, key: String, vm: &VirtualMachine) -> PyResult<()> {
            if !valid_ident(&key) {
                return Err(vm.new_value_error(format!("invalid setting key '{key}'")));
            }
            zelf.remove_raw(&key);
            Ok(())
        }

        /// The keys this namespace owns (no nested ones).
        #[pymethod]
        fn keys(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            let items: Vec<PyObjectRef> = zelf
                .names()
                .into_iter()
                .map(|name| vm.new_pyobj(name))
                .collect();
            vm.ctx.new_list(items).into()
        }
    }

    fn document_py(row: DocumentRow, vm: &VirtualMachine) -> PyObjectRef {
        PyDocument { row }.into_ref(&vm.ctx).into()
    }

    /// One object instance, with its property values in its instance dict.
    ///
    /// Native types do not reach a Python `__getattr__` from `get_attr` here, so the
    /// payload's values are seeded into the object's dict instead; a normal attribute
    /// lookup finds them. `Label` is seeded beside them because it has no getter — the
    /// dict is where a rename lands.
    fn object_py(row: ObjectRow, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
        let name = row.name.clone();
        let label = row.label.clone();
        let values: Vec<(String, String)> = row
            .properties
            .iter()
            .map(|property| (property.name.clone(), property.value.clone()))
            .collect();
        let object: PyObjectRef = PyDocumentObject { row }.into_ref(&vm.ctx).into();
        if let Some(dict) = object.dict() {
            dict.set_item("Label", vm.new_pyobj(label), vm)?;
            for (name, value) in values {
                dict.set_item(&*name, vm.new_pyobj(value), vm)?;
            }
            // A `Shape` is a B-Rep, not text: hand the script the kernel handle the
            // document holds (B18b item 2), not the payload's placeholder string.
            if let Some(shape) = crate::native_shapes::shape_py_of(&name, vm) {
                dict.set_item("Shape", shape, vm)?;
            }
        }
        Ok(object)
    }

    /// An `App.Vector` for a Rust vector (the value type `Part` returns).
    pub(crate) fn vector_py(value: Vector3, vm: &VirtualMachine) -> PyObjectRef {
        PyVector { inner: value }.into_ref(&vm.ctx).into()
    }

    /// The vector a Python value is, when it is an `App.Vector`.
    pub(crate) fn vector_of(value: &PyObjectRef) -> Option<Vector3> {
        value.downcast_ref::<PyVector>().map(|vector| vector.inner)
    }

    /// An `App.BoundBox` from a min/max pair (a `Part.Shape`'s bounds).
    pub(crate) fn bound_box_py(min: Vector3, max: Vector3, vm: &VirtualMachine) -> PyObjectRef {
        PyBoundBox {
            inner: RefCell::new(Box3 {
                min: [min.x, min.y, min.z],
                max: [max.x, max.y, max.z],
                valid: true,
            }),
        }
        .into_ref(&vm.ctx)
        .into()
    }

    /// A document object by name, from the fresh payload.
    pub(crate) fn object_named_py(name: &str, vm: &VirtualMachine) -> Option<PyObjectRef> {
        object_named(name, vm)
    }

    /// The object the fresh payload names, as a Python instance, or `None`.
    fn object_named(name: &str, vm: &VirtualMachine) -> Option<PyObjectRef> {
        let row = active_row()?;
        object_py(row.objects.into_iter().find(|row| row.name == name)?, vm).ok()
    }

    /// Refresh an object's instance dict from the payload after a write.
    ///
    /// A write recomputes, so rows other than the one assigned can change; replacing the
    /// whole dict keeps the object a script already holds agreeing with the document.
    fn reseed(object: &Py<PyDocumentObject>, vm: &VirtualMachine) -> PyResult<()> {
        let Some(row) = active_row() else {
            return Ok(());
        };
        let Some(fresh) = row
            .objects
            .into_iter()
            .find(|row| row.name == object.row.name)
        else {
            return Ok(());
        };
        let object_ref: PyObjectRef = object.to_owned().into();
        if let Some(dict) = object_ref.dict() {
            dict.set_item("Label", vm.new_pyobj(fresh.label), vm)?;
            for property in fresh.properties {
                dict.set_item(&*property.name, vm.new_pyobj(property.value), vm)?;
            }
            if let Some(shape) = crate::native_shapes::shape_py_of(&fresh.name, vm) {
                dict.set_item("Shape", shape, vm)?;
            }
        }
        Ok(())
    }

    fn read_only(what: &str, vm: &VirtualMachine) -> rustpython_vm::builtins::PyBaseExceptionRef {
        vm.new_runtime_error(format!(
            "pywasm: {what} is the shell's; this host's document is read-only from Python"
        ))
    }

    /// The active document, read fresh on every call (`None` when nothing is open).
    #[pyfunction]
    pub fn activeDocument(vm: &VirtualMachine) -> PyObjectRef {
        match active_row() {
            Some(row) => document_py(row, vm),
            None => vm.ctx.none(),
        }
    }

    /// The open document known by `name`, or `None`.
    #[pyfunction]
    pub fn getDocument(name: String, vm: &VirtualMachine) -> PyObjectRef {
        match active_row().filter(|row| row.name == name) {
            Some(row) => document_py(row, vm),
            None => vm.ctx.none(),
        }
    }

    /// Every open document, keyed by name (upstream returns a dict).
    #[pyfunction]
    pub fn listDocuments(vm: &VirtualMachine) -> PyDictRef {
        let dict = vm.ctx.new_dict();
        if let Some(row) = active_row() {
            let name = row.name.clone();
            dict.set_item(&*name, document_py(row, vm), vm)
                .expect("a document is a valid dict value");
        }
        dict
    }

    /// Creating a document belongs to the document half (the shell owns the registry).
    #[pyfunction]
    pub fn newDocument(_name: PyObjectRef, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
        Err(read_only("creating a document", vm))
    }

    #[pyfunction]
    pub fn openDocument(_path: PyObjectRef, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
        Err(read_only("opening a document", vm))
    }

    #[pyfunction]
    pub fn closeDocument(_name: PyObjectRef, vm: &VirtualMachine) -> PyResult<()> {
        Err(read_only("closing a document", vm))
    }

    /// Accepts the already-active name (a no-op); anything else is the shell's.
    #[pyfunction]
    pub fn setActiveDocument(name: String, vm: &VirtualMachine) -> PyResult<()> {
        match active_row() {
            Some(row) if row.name == name => Ok(()),
            _ => Err(read_only("activating another document", vm)),
        }
    }

    /// Stored, not pumped: the shell delivers document events to `FerroCADGui`.
    #[pyfunction]
    pub fn addDocumentObserver(observer: PyObjectRef) {
        OBSERVERS.with(|slot| slot.borrow_mut().push(observer));
    }

    /// The inverse of [`addDocumentObserver`].
    #[pyfunction]
    pub fn removeDocumentObserver(observer: PyObjectRef) {
        OBSERVERS.with(|slot| {
            slot.borrow_mut()
                .retain(|item| !std::ptr::eq(&**item, &*observer))
        });
    }

    // --- the parameter file -------------------------------------------------
    //
    // The store itself is `python/FreeCAD`'s; the host supplies the two file
    // operations, so a build without a POSIX layer can still persist settings (B18b).

    /// The parameter file's text, or `None` when it does not exist.
    #[pyfunction]
    pub fn read_parameter_file(path: String, vm: &VirtualMachine) -> PyResult<Option<String>> {
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(vm.new_os_error(format!("cannot read {path}: {error}"))),
        }
    }

    /// Replace the parameter file with `text`, creating its directory.
    ///
    /// Written beside the target and renamed, so a crash cannot leave a half file in
    /// place (the facade's report-once rule then covers a real write failure).
    #[pyfunction]
    pub fn write_parameter_file(path: String, text: String, vm: &VirtualMachine) -> PyResult<()> {
        let target = std::path::Path::new(&path);
        let parent = match target.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => std::path::PathBuf::from("."),
        };
        std::fs::create_dir_all(&parent).map_err(|error| {
            vm.new_os_error(format!("cannot create {}: {error}", parent.display()))
        })?;
        let temporary = parent.join(format!(".parameters-{}.tmp", std::process::id()));
        let write = (|| -> std::io::Result<()> {
            use std::io::Write;
            let mut file = std::fs::File::create(&temporary)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            std::fs::rename(&temporary, target)
        })();
        if let Err(error) = write {
            let _ = std::fs::remove_file(&temporary);
            return Err(vm.new_os_error(format!("cannot write {path}: {error}")));
        }
        Ok(())
    }

    // --- the property rules `ferrocad_shell` reads -------------------------

    #[pyfunction]
    pub fn propertyKindOf(
        object: PyRef<PyDocumentObject>,
        prop: String,
        vm: &VirtualMachine,
    ) -> PyResult<String> {
        object
            .row
            .property(&prop)
            .map(|row| row.kind.clone())
            .ok_or_else(|| attribute_error(&prop, vm))
    }

    #[pyfunction]
    pub fn formatProperty(
        object: PyRef<PyDocumentObject>,
        prop: String,
        vm: &VirtualMachine,
    ) -> PyResult<String> {
        object
            .row
            .property(&prop)
            .map(|row| row.value.clone())
            .ok_or_else(|| attribute_error(&prop, vm))
    }

    /// Text passed through: the coercion is the doorway's, on the write itself.
    ///
    /// The facade's `set_property` uses this to validate before it assigns; here the
    /// doorway's `set` runs the same `ferrocad_core::present` coercion on the editor text
    /// (and refuses with a Python exception when it cannot), so coercing twice would only
    /// add a second spelling of the rules.
    #[pyfunction]
    pub fn coercePropertyText(
        _object: PyRef<PyDocumentObject>,
        _prop: String,
        text: String,
    ) -> String {
        text
    }

    // --- value types -------------------------------------------------------

    /// `FreeCAD.Units.Quantity`, over `ferrocad_core`'s parser and unit table.
    #[pyattr]
    #[pyclass(name = "Quantity", module = "ferrocad")]
    #[derive(Debug, PyPayload)]
    pub struct PyQuantity {
        inner: Quantity,
    }

    impl Constructor for PyQuantity {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, args: Self::Args, vm: &VirtualMachine) -> PyResult<Self> {
            let mut positional = args.args;
            match positional.len() {
                0 => Ok(Self {
                    inner: Quantity::dimensionless(0.0),
                }),
                1 => {
                    let value = positional.remove(0);
                    let inner = if let Some(copy) = value.downcast_ref::<Self>() {
                        copy.inner
                    } else if let Ok(text) = value.try_to_value::<String>(vm) {
                        Quantity::parse(&text).map_err(|error| vm.new_value_error(error))?
                    } else {
                        Quantity::dimensionless(number(&value, vm)?)
                    };
                    Ok(Self { inner })
                }
                2 => {
                    let number = number(&positional[0], vm)?;
                    let symbol = positional[1].try_to_value::<String>(vm)?;
                    let unit = Unit::parse(&symbol)
                        .ok_or_else(|| vm.new_value_error(format!("unknown unit '{symbol}'")))?;
                    Ok(Self {
                        inner: Quantity::new(number, unit),
                    })
                }
                _ => Err(vm.new_type_error("Quantity() takes 0, 1 or 2 arguments")),
            }
        }
    }

    #[pyclass(with(Constructor))]
    impl PyQuantity {
        #[pygetset]
        fn Value(zelf: &Py<Self>) -> f64 {
            zelf.inner.value()
        }

        #[pymethod]
        fn getValueAs(zelf: &Py<Self>, unit: String, vm: &VirtualMachine) -> PyResult<Self> {
            let unit = Unit::parse(&unit)
                .ok_or_else(|| vm.new_value_error(format!("unknown unit '{unit}'")))?;
            if unit.sig != zelf.inner.sig() {
                return Err(vm.new_value_error("incompatible unit for getValueAs"));
            }
            Ok(Self {
                inner: zelf.inner.in_unit(unit),
            })
        }

        #[pymethod]
        fn __str__(zelf: &Py<Self>) -> String {
            zelf.inner.user_string()
        }
    }

    /// `FreeCAD.Base.Unit`, over the engine's unit table.
    #[pyattr]
    #[pyclass(name = "Unit", module = "ferrocad")]
    #[derive(Debug, PyPayload)]
    pub struct PyUnit {
        inner: Unit,
    }

    impl Constructor for PyUnit {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, args: Self::Args, vm: &VirtualMachine) -> PyResult<Self> {
            let symbol = match args.args.first() {
                Some(value) => value.clone().try_into_value::<String>(vm)?,
                None => String::new(),
            };
            let inner = Unit::parse(&symbol)
                .ok_or_else(|| vm.new_value_error(format!("unknown unit '{symbol}'")))?;
            Ok(Self { inner })
        }
    }

    #[pyclass(with(Constructor))]
    impl PyUnit {
        #[pymethod]
        fn __str__(zelf: &Py<Self>) -> String {
            canonical_name(zelf.inner.sig)
        }
    }

    /// `FreeCAD.Vector`, over `ferrocad_types::geometry::Vector3`.
    #[pyattr]
    #[pyclass(name = "Vector", module = "ferrocad")]
    #[derive(Debug, PyPayload, Clone, Copy)]
    pub struct PyVector {
        inner: Vector3,
    }

    impl Constructor for PyVector {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, args: Self::Args, vm: &VirtualMachine) -> PyResult<Self> {
            let positional = args.args;
            let inner = match positional.len() {
                0 => Vector3::zero(),
                1 => match positional[0].downcast_ref::<Self>() {
                    Some(copy) => copy.inner,
                    None => return Err(vm.new_type_error("Vector() takes 0, 1 or 3 arguments")),
                },
                3 => Vector3::new(
                    number(&positional[0], vm)?,
                    number(&positional[1], vm)?,
                    number(&positional[2], vm)?,
                ),
                _ => return Err(vm.new_type_error("Vector() takes 0, 1 or 3 arguments")),
            };
            Ok(Self { inner })
        }
    }

    #[pyclass(with(Constructor))]
    impl PyVector {
        #[pygetset]
        fn x(zelf: &Py<Self>) -> f64 {
            zelf.inner.x
        }

        #[pygetset]
        fn y(zelf: &Py<Self>) -> f64 {
            zelf.inner.y
        }

        #[pygetset]
        fn z(zelf: &Py<Self>) -> f64 {
            zelf.inner.z
        }

        #[pygetset]
        fn Length(zelf: &Py<Self>) -> f64 {
            zelf.inner.length()
        }

        #[pymethod]
        fn add(zelf: &Py<Self>, other: PyRef<PyVector>) -> Self {
            Self {
                inner: zelf.inner.add(&other.inner),
            }
        }

        #[pymethod]
        fn sub(zelf: &Py<Self>, other: PyRef<PyVector>) -> Self {
            Self {
                inner: zelf.inner.sub(&other.inner),
            }
        }

        #[pymethod]
        fn scale(zelf: &Py<Self>, factor: f64) -> Self {
            Self {
                inner: zelf.inner.scale(factor),
            }
        }

        #[pymethod]
        fn dot(zelf: &Py<Self>, other: PyRef<PyVector>) -> f64 {
            zelf.inner.dot(&other.inner)
        }

        #[pymethod]
        fn cross(zelf: &Py<Self>, other: PyRef<PyVector>) -> Self {
            Self {
                inner: zelf.inner.cross(&other.inner),
            }
        }

        #[pymethod]
        fn normalize(zelf: &Py<Self>) -> Self {
            Self {
                inner: zelf.inner.normalize(),
            }
        }

        #[pymethod]
        fn __str__(zelf: &Py<Self>) -> String {
            format!(
                "Vector ({}, {}, {})",
                zelf.inner.x, zelf.inner.y, zelf.inner.z
            )
        }
    }

    /// `FreeCAD.Vector2d`.
    #[pyattr]
    #[pyclass(name = "Vector2d", module = "ferrocad")]
    #[derive(Debug, PyPayload, Clone, Copy)]
    pub struct PyVector2d {
        x: f64,
        y: f64,
    }

    impl Constructor for PyVector2d {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, args: Self::Args, vm: &VirtualMachine) -> PyResult<Self> {
            match args.args.len() {
                0 => Ok(Self { x: 0.0, y: 0.0 }),
                2 => Ok(Self {
                    x: number(&args.args[0], vm)?,
                    y: number(&args.args[1], vm)?,
                }),
                _ => Err(vm.new_type_error("Vector2d() takes 0 or 2 arguments")),
            }
        }
    }

    #[pyclass(with(Constructor))]
    impl PyVector2d {
        #[pygetset]
        fn x(zelf: &Py<Self>) -> f64 {
            zelf.x
        }

        #[pygetset]
        fn y(zelf: &Py<Self>) -> f64 {
            zelf.y
        }
    }

    /// FreeCAD's `A<row><col>` element name (1-based) as a row-major index.
    fn matrix_index(name: &str) -> Option<usize> {
        let bytes = name.as_bytes();
        if bytes.len() == 3
            && bytes[0] == b'A'
            && (b'1'..=b'4').contains(&bytes[1])
            && (b'1'..=b'4').contains(&bytes[2])
        {
            Some((bytes[1] - b'1') as usize * 4 + (bytes[2] - b'1') as usize)
        } else {
            None
        }
    }

    /// `FreeCAD.Matrix`: the 4x4 transform, addressed by `A11`…`A44`.
    #[pyattr]
    #[pyclass(name = "Matrix", module = "ferrocad")]
    #[derive(Debug, PyPayload)]
    pub struct PyMatrix {
        inner: RefCell<Matrix4>,
    }

    impl Constructor for PyMatrix {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, _args: Self::Args, _vm: &VirtualMachine) -> PyResult<Self> {
            Ok(Self {
                inner: RefCell::new(Matrix4::identity()),
            })
        }
    }

    #[pyclass(with(Constructor))]
    impl PyMatrix {
        #[pymethod]
        fn __getattr__(zelf: &Py<Self>, name: PyStrRef, vm: &VirtualMachine) -> PyResult<f64> {
            let name = name.to_str().unwrap_or_default();
            matrix_index(name)
                .map(|index| zelf.inner.borrow().m[index])
                .ok_or_else(|| attribute_error(name, vm))
        }

        #[pymethod]
        fn __setattr__(
            zelf: &Py<Self>,
            name: PyStrRef,
            value: f64,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            let name = name.to_str().unwrap_or_default();
            let index = matrix_index(name).ok_or_else(|| attribute_error(name, vm))?;
            zelf.inner.borrow_mut().m[index] = value;
            Ok(())
        }
    }

    /// `FreeCAD.Rotation`.
    #[pyattr]
    #[pyclass(name = "Rotation", module = "ferrocad")]
    #[derive(Debug, PyPayload, Clone, Copy)]
    pub struct PyRotation {
        inner: Rotation,
    }

    impl Constructor for PyRotation {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, args: Self::Args, vm: &VirtualMachine) -> PyResult<Self> {
            let positional = args.args;
            let inner = match positional.len() {
                0 => Rotation::identity(),
                1 => {
                    let matrix = positional[0]
                        .clone()
                        .try_into_value::<PyRef<PyMatrix>>(vm)?;
                    Rotation::from_matrix(&matrix.inner.borrow())
                }
                2 => {
                    let axis = positional[0]
                        .clone()
                        .try_into_value::<PyRef<PyVector>>(vm)?;
                    let angle = number(&positional[1], vm)?;
                    Rotation::from_axis_angle(&axis.inner, angle)
                }
                _ => return Err(vm.new_type_error("Rotation() takes 0, 1 or 2 arguments")),
            };
            Ok(Self { inner })
        }
    }

    #[pyclass(with(Constructor))]
    impl PyRotation {
        #[pygetset]
        fn Angle(zelf: &Py<Self>) -> f64 {
            zelf.inner.angle()
        }

        #[pygetset]
        fn Axis(zelf: &Py<Self>) -> PyVector {
            PyVector {
                inner: zelf.inner.axis(),
            }
        }

        #[pymethod]
        fn multVec(zelf: &Py<Self>, vector: PyRef<PyVector>) -> PyVector {
            PyVector {
                inner: zelf.inner.to_matrix().transform(&vector.inner),
            }
        }
    }

    /// `FreeCAD.Placement`.
    #[pyattr]
    #[pyclass(name = "Placement", module = "ferrocad")]
    #[derive(Debug, PyPayload, Clone, Copy)]
    pub struct PyPlacement {
        inner: Placement,
    }

    impl Constructor for PyPlacement {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, args: Self::Args, vm: &VirtualMachine) -> PyResult<Self> {
            let positional = args.args;
            let inner = match positional.len() {
                0 => Placement::identity(),
                // `Placement(base, rotation)`; `Placement(matrix)` (C++'s three-vector
                // form) is not used by the shared code.
                2 => {
                    let base = positional[0]
                        .clone()
                        .try_into_value::<PyRef<PyVector>>(vm)?;
                    let rotation = positional[1]
                        .clone()
                        .try_into_value::<PyRef<PyRotation>>(vm)?;
                    Placement::new(base.inner, rotation.inner)
                }
                _ => return Err(vm.new_type_error("Placement() takes 0 or 2 arguments")),
            };
            Ok(Self { inner })
        }
    }

    #[pyclass(with(Constructor))]
    impl PyPlacement {
        #[pygetset]
        fn Base(zelf: &Py<Self>) -> PyVector {
            PyVector {
                inner: zelf.inner.base,
            }
        }

        #[pygetset]
        fn Rotation(zelf: &Py<Self>) -> PyRotation {
            PyRotation {
                inner: zelf.inner.rotation,
            }
        }

        #[pymethod]
        fn multVec(zelf: &Py<Self>, vector: PyRef<PyVector>) -> PyVector {
            PyVector {
                inner: zelf.inner.to_matrix().transform(&vector.inner),
            }
        }

        #[pymethod]
        fn inverse(zelf: &Py<Self>) -> Self {
            Self {
                inner: zelf.inner.inverse(),
            }
        }
    }

    /// `FreeCAD.TypeId`: an interned type name.
    #[pyattr]
    #[pyclass(name = "TypeId", module = "ferrocad")]
    #[derive(Debug, PyPayload, Clone, PartialEq, Eq)]
    pub struct PyTypeId {
        name: String,
    }

    impl Constructor for PyTypeId {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, args: Self::Args, vm: &VirtualMachine) -> PyResult<Self> {
            let name = match args.args.first() {
                Some(value) => value.clone().try_into_value::<String>(vm)?,
                None => String::new(),
            };
            Ok(Self { name })
        }
    }

    #[pyclass(with(Constructor))]
    impl PyTypeId {
        #[pymethod]
        fn __str__(zelf: &Py<Self>) -> String {
            zelf.name.clone()
        }
    }

    /// `FreeCAD.BoundBox`: an axis-aligned box, valid once it spans a point.
    #[pyattr]
    #[pyclass(name = "BoundBox", module = "ferrocad")]
    #[derive(Debug, PyPayload)]
    pub struct PyBoundBox {
        inner: RefCell<Box3>,
    }

    #[derive(Debug, Clone, Copy)]
    struct Box3 {
        min: [f64; 3],
        max: [f64; 3],
        valid: bool,
    }

    impl Box3 {
        fn empty() -> Self {
            Self {
                min: [0.0; 3],
                max: [0.0; 3],
                valid: false,
            }
        }
    }

    impl Constructor for PyBoundBox {
        type Args = FuncArgs;

        /// `BoundBox()`, `BoundBox(other)` or `BoundBox(xmin, ymin, zmin, xmax, ymax, zmax)`
        /// — upstream's three forms that carry no sequence coercion.
        fn py_new(_cls: &Py<PyType>, args: Self::Args, vm: &VirtualMachine) -> PyResult<Self> {
            let positional = args.args;
            let inner = match positional.len() {
                0 => Box3::empty(),
                1 => match positional[0].downcast_ref::<Self>() {
                    Some(copy) => *copy.inner.borrow(),
                    None => {
                        return Err(vm.new_type_error("BoundBox() takes 0, 1 or 6 arguments"));
                    }
                },
                6 => Box3 {
                    min: [
                        number(&positional[0], vm)?,
                        number(&positional[1], vm)?,
                        number(&positional[2], vm)?,
                    ],
                    max: [
                        number(&positional[3], vm)?,
                        number(&positional[4], vm)?,
                        number(&positional[5], vm)?,
                    ],
                    valid: true,
                },
                _ => return Err(vm.new_type_error("BoundBox() takes 0, 1 or 6 arguments")),
            };
            Ok(Self {
                inner: RefCell::new(inner),
            })
        }
    }

    #[pyclass(with(Constructor))]
    impl PyBoundBox {
        #[pygetset]
        fn XMin(zelf: &Py<Self>) -> f64 {
            zelf.inner.borrow().min[0]
        }

        #[pygetset]
        fn YMin(zelf: &Py<Self>) -> f64 {
            zelf.inner.borrow().min[1]
        }

        #[pygetset]
        fn ZMin(zelf: &Py<Self>) -> f64 {
            zelf.inner.borrow().min[2]
        }

        #[pygetset]
        fn XMax(zelf: &Py<Self>) -> f64 {
            zelf.inner.borrow().max[0]
        }

        #[pygetset]
        fn YMax(zelf: &Py<Self>) -> f64 {
            zelf.inner.borrow().max[1]
        }

        #[pygetset]
        fn ZMax(zelf: &Py<Self>) -> f64 {
            zelf.inner.borrow().max[2]
        }

        #[pygetset]
        fn isValid(zelf: &Py<Self>) -> bool {
            zelf.inner.borrow().valid
        }

        /// Grow the box by `amount` in every direction.
        #[pymethod]
        fn enlarge(zelf: &Py<Self>, amount: f64) {
            let mut inner = zelf.inner.borrow_mut();
            if inner.valid {
                for axis in 0..3 {
                    inner.min[axis] -= amount;
                    inner.max[axis] += amount;
                }
            }
        }
    }

    /// `FreeCAD.Material`: the renderer's appearance values.
    #[pyattr]
    #[pyclass(name = "Material", module = "ferrocad")]
    #[derive(Debug, PyPayload)]
    pub struct PyMaterial {
        inner: RefCell<ferrocad_core::Material>,
    }

    impl Constructor for PyMaterial {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, _args: Self::Args, _vm: &VirtualMachine) -> PyResult<Self> {
            Ok(Self {
                inner: RefCell::new(ferrocad_core::Material::default()),
            })
        }
    }

    /// A four-number colour (`DiffuseColor = (r, g, b, a)`).
    fn colour(value: &PyObjectRef, vm: &VirtualMachine) -> PyResult<[f64; 4]> {
        let channels: Vec<f64> = value.clone().try_into_value::<Vec<f64>>(vm)?;
        if channels.len() != 4 {
            return Err(vm.new_value_error("a colour takes 4 numbers (r, g, b, a)"));
        }
        Ok([channels[0], channels[1], channels[2], channels[3]])
    }

    #[pyclass(with(Constructor))]
    impl PyMaterial {
        #[pygetset]
        fn DiffuseColor(zelf: &Py<Self>) -> (f64, f64, f64, f64) {
            tuple(zelf.inner.borrow().diffuse)
        }

        #[pygetset(setter)]
        fn set_DiffuseColor(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            zelf.inner.borrow_mut().diffuse = colour(&value, vm)?;
            Ok(())
        }

        #[pygetset]
        fn AmbientColor(zelf: &Py<Self>) -> (f64, f64, f64, f64) {
            tuple(zelf.inner.borrow().ambient)
        }

        #[pygetset(setter)]
        fn set_AmbientColor(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            zelf.inner.borrow_mut().ambient = colour(&value, vm)?;
            Ok(())
        }

        #[pygetset]
        fn SpecularColor(zelf: &Py<Self>) -> (f64, f64, f64, f64) {
            tuple(zelf.inner.borrow().specular)
        }

        #[pygetset(setter)]
        fn set_SpecularColor(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            zelf.inner.borrow_mut().specular = colour(&value, vm)?;
            Ok(())
        }

        #[pygetset]
        fn EmissiveColor(zelf: &Py<Self>) -> (f64, f64, f64, f64) {
            tuple(zelf.inner.borrow().emissive)
        }

        #[pygetset(setter)]
        fn set_EmissiveColor(
            zelf: &Py<Self>,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            zelf.inner.borrow_mut().emissive = colour(&value, vm)?;
            Ok(())
        }

        #[pygetset]
        fn Shininess(zelf: &Py<Self>) -> f64 {
            zelf.inner.borrow().shininess
        }

        #[pygetset(setter)]
        fn set_Shininess(zelf: &Py<Self>, value: f64) {
            zelf.inner.borrow_mut().shininess = value;
        }

        #[pygetset]
        fn Transparency(zelf: &Py<Self>) -> f64 {
            zelf.inner.borrow().transparency
        }

        #[pygetset(setter)]
        fn set_Transparency(zelf: &Py<Self>, value: f64) {
            zelf.inner.borrow_mut().transparency = value;
        }
    }

    fn tuple(colour: [f64; 4]) -> (f64, f64, f64, f64) {
        (colour[0], colour[1], colour[2], colour[3])
    }

    /// `FreeCAD.StringHasher`: the interned-string table.
    #[pyattr]
    #[pyclass(name = "StringHasher", module = "ferrocad")]
    #[derive(PyPayload)]
    pub struct PyStringHasher {
        inner: ferrocad_core::StringHasher,
    }

    impl std::fmt::Debug for PyStringHasher {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("StringHasher")
        }
    }

    impl Constructor for PyStringHasher {
        type Args = FuncArgs;

        fn py_new(_cls: &Py<PyType>, _args: Self::Args, _vm: &VirtualMachine) -> PyResult<Self> {
            Ok(Self {
                inner: ferrocad_core::StringHasher::new(),
            })
        }
    }

    #[pyclass(with(Constructor))]
    impl PyStringHasher {
        /// The `StringID` for `text`, or the one with `value` when given an int.
        #[pymethod]
        fn getID(zelf: &Py<Self>, arg: PyObjectRef, vm: &VirtualMachine) -> PyResult<PyStringID> {
            if let Ok(text) = arg.clone().try_into_value::<String>(vm) {
                return Ok(PyStringID {
                    inner: zelf.inner.get_id(&text),
                });
            }
            if let Ok(value) = arg.clone().try_into_value::<usize>(vm) {
                return zelf
                    .inner
                    .find_id(value)
                    .map(|inner| PyStringID { inner })
                    .ok_or_else(|| vm.new_value_error(format!("no StringID with value {value}")));
            }
            Err(vm.new_value_error("getID expects a string or an integer"))
        }

        #[pymethod]
        fn isSame(zelf: &Py<Self>, other: PyRef<Self>) -> bool {
            zelf.inner.is_same(&other.inner)
        }
    }

    /// `FreeCAD.StringID`: a handle into a `StringHasher`.
    #[pyattr]
    #[pyclass(name = "StringID", module = "ferrocad")]
    #[derive(PyPayload, Clone)]
    pub struct PyStringID {
        inner: ferrocad_core::StringId,
    }

    impl std::fmt::Debug for PyStringID {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "StringID({})", self.inner.value())
        }
    }

    #[pyclass]
    impl PyStringID {
        #[pygetset]
        fn Value(zelf: &Py<Self>) -> usize {
            zelf.inner.value()
        }

        #[pygetset]
        fn Data(zelf: &Py<Self>) -> String {
            zelf.inner.data()
        }

        #[pymethod]
        fn isSame(zelf: &Py<Self>, other: PyRef<Self>) -> bool {
            zelf.inner.is_same(&other.inner)
        }
    }
}
