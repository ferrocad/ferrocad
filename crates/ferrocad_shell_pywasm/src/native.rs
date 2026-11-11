//! The `ferrocad` native module RustPython imports.
//!
//! `python/FreeCAD` (and `python/ferrocad_shell`'s `_fc` calls) use this binding. It is
//! the one piece that cannot be shared with the CPython half: `ferrocad_py` is a PyO3
//! extension, and a `wasm32-unknown-unknown` interpreter has no C ABI to load it with.
//! The *types and rules* are `ferrocad_core`'s; only the binding is per-VM.
//!
//! **The document is read-only.** `activeDocument`/`getDocument`/`listDocuments` read
//! the payload `ferrocad_doc_api::payload` renders from the document the host holds, and
//! answer with real `Document`/`DocumentObject` instances. Creating, opening and closing
//! a document raise: the document half owns those, and the shell reaches them through
//! its own bridge.
//!
//! **Value types.** `Quantity`, `Unit`, `Vector`, `Vector2d`, `Matrix`, `Rotation`,
//! `Placement` and `TypeId` are bound over the same `ferrocad_types` structures the
//! CPython module uses, so a workplane computation in a browser console means what it
//! means natively. The remaining value classes `python/FreeCAD` re-exports (`BoundBox`,
//! `Material`, `StringID`, `StringHasher`) are not bound here; their absence is recorded
//! in the B18b ledger rather than papered over with empty classes.
//!
//! **Document observers** are stored, not pumped: this half has no document-event loop,
//! and the shell delivers document events to `FerroCADGui` instead.

pub(crate) use ferrocad::module_def;

use std::cell::RefCell;

use ferrocad_core::geometry::{Matrix4, Placement, Rotation, Vector3};
use ferrocad_core::quantity::{Quantity, canonical_name};
use ferrocad_core::unit::Unit;
use rustpython_vm::builtins::{PyDictRef, PyStr, PyStrRef, PyType};
use rustpython_vm::function::{FuncArgs, PySetterValue};
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

thread_local! {
    /// Document observers a script registered. See the module docs: stored, not pumped.
    static OBSERVERS: Observers = const { RefCell::new(Vec::new()) };
}

#[pymodule]
mod ferrocad {
    #![allow(non_snake_case, non_upper_case_globals)]

    use super::{
        Constructor, DocumentRow, FuncArgs, Matrix4, OBSERVERS, ObjectRow, Placement, Py,
        PyDictRef, PyObjectRef, PyPayload, PyRef, PyResult, PySetterValue, PyStr, PyStrRef, PyType,
        Quantity, Rotation, SetAttr, Unit, Vector3, VirtualMachine, active_row, canonical_name,
        pyclass,
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
        fn Name(&self) -> String {
            self.row.name.clone()
        }

        #[pygetset]
        fn Label(&self) -> String {
            self.row.label.clone()
        }

        #[pygetset]
        fn TypeId(&self) -> String {
            self.row.type_id.clone()
        }

        #[pygetset]
        fn PropertiesList(&self, vm: &VirtualMachine) -> PyObjectRef {
            let names: Vec<PyObjectRef> = self
                .row
                .properties
                .iter()
                .map(|row| vm.new_pyobj(row.name.clone()))
                .collect();
            vm.ctx.new_list(names).into()
        }

        /// `[(name, expression)]`; no expressions are stored in this projection.
        #[pygetset]
        fn ExpressionEngine(&self, vm: &VirtualMachine) -> PyObjectRef {
            let pairs: Vec<PyObjectRef> = self
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
        fn getTypeOfProperty(&self, prop: String, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let row = self
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
        fn getTypeIdOfProperty(&self, prop: String, vm: &VirtualMachine) -> PyResult<String> {
            self.row
                .property(&prop)
                .map(|row| row.type_id.clone())
                .ok_or_else(|| attribute_error(&prop, vm))
        }

        #[pymethod]
        fn getEnumerationsOfProperty(
            &self,
            prop: String,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            let row = self
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

        /// A property's display value, as the payload rendered it.
        #[pymethod]
        fn __getattr__(&self, name: PyStrRef, vm: &VirtualMachine) -> PyResult<String> {
            let name = name.to_str().unwrap_or_default();
            self.row
                .property(name)
                .map(|row| row.value.clone())
                .ok_or_else(|| attribute_error(name, vm))
        }
    }

    /// Read-only: the document half owns the write path.
    impl SetAttr for PyDocumentObject {
        fn setattro(
            _zelf: &Py<Self>,
            _name: &Py<PyStr>,
            _value: PySetterValue,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            Err(read_only("editing a property", vm))
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
        fn Name(&self) -> String {
            self.row.name.clone()
        }

        #[pygetset]
        fn Label(&self) -> String {
            self.row.name.clone()
        }

        #[pygetset]
        fn Objects(&self, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let mut objects = Vec::with_capacity(self.row.objects.len());
            for row in self.row.objects.iter().cloned() {
                objects.push(object_py(row, vm)?);
            }
            Ok(vm.ctx.new_list(objects).into())
        }

        #[pygetset]
        fn UndoCount(&self) -> i64 {
            self.row.undo_count
        }

        #[pygetset]
        fn RedoCount(&self) -> i64 {
            self.row.redo_count
        }

        #[pymethod]
        fn getObject(&self, name: String, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            match self
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
    }

    fn document_py(row: DocumentRow, vm: &VirtualMachine) -> PyObjectRef {
        PyDocument { row }.into_ref(&vm.ctx).into()
    }

    /// One object instance, with its property values in its instance dict.
    ///
    /// Native types do not reach a Python `__getattr__` from `get_attr` here, so the
    /// payload's values are seeded into the object's dict instead; a normal attribute
    /// lookup finds them, and `__setattr__` still refuses a write.
    fn object_py(row: ObjectRow, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
        let values: Vec<(String, String)> = row
            .properties
            .iter()
            .map(|property| (property.name.clone(), property.value.clone()))
            .collect();
        let object: PyObjectRef = PyDocumentObject { row }.into_ref(&vm.ctx).into();
        if let Some(dict) = object.dict() {
            for (name, value) in values {
                dict.set_item(&*name, vm.new_pyobj(value), vm)?;
            }
        }
        Ok(object)
    }

    fn read_only(what: &str, vm: &VirtualMachine) -> rustpython_vm::builtins::PyBaseExceptionRef {
        vm.new_runtime_error(format!(
            "pywasm: {what} is the shell's; the document is read-only from Python"
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

    /// Coercion is the engine's, and its write path is not reachable from this half,
    /// so the text is accepted verbatim (see the module docs).
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
        fn Value(&self) -> f64 {
            self.inner.value()
        }

        #[pymethod]
        fn getValueAs(&self, unit: String, vm: &VirtualMachine) -> PyResult<Self> {
            let unit = Unit::parse(&unit)
                .ok_or_else(|| vm.new_value_error(format!("unknown unit '{unit}'")))?;
            if unit.sig != self.inner.sig() {
                return Err(vm.new_value_error("incompatible unit for getValueAs"));
            }
            Ok(Self {
                inner: self.inner.in_unit(unit),
            })
        }

        #[pymethod]
        fn __str__(&self) -> String {
            self.inner.user_string()
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
        fn __str__(&self) -> String {
            canonical_name(self.inner.sig)
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
        fn x(&self) -> f64 {
            self.inner.x
        }

        #[pygetset]
        fn y(&self) -> f64 {
            self.inner.y
        }

        #[pygetset]
        fn z(&self) -> f64 {
            self.inner.z
        }

        #[pygetset]
        fn Length(&self) -> f64 {
            self.inner.length()
        }

        #[pymethod]
        fn add(&self, other: PyRef<PyVector>) -> Self {
            Self {
                inner: self.inner.add(&other.inner),
            }
        }

        #[pymethod]
        fn sub(&self, other: PyRef<PyVector>) -> Self {
            Self {
                inner: self.inner.sub(&other.inner),
            }
        }

        #[pymethod]
        fn scale(&self, factor: f64) -> Self {
            Self {
                inner: self.inner.scale(factor),
            }
        }

        #[pymethod]
        fn dot(&self, other: PyRef<PyVector>) -> f64 {
            self.inner.dot(&other.inner)
        }

        #[pymethod]
        fn cross(&self, other: PyRef<PyVector>) -> Self {
            Self {
                inner: self.inner.cross(&other.inner),
            }
        }

        #[pymethod]
        fn normalize(&self) -> Self {
            Self {
                inner: self.inner.normalize(),
            }
        }

        #[pymethod]
        fn __str__(&self) -> String {
            format!(
                "Vector ({}, {}, {})",
                self.inner.x, self.inner.y, self.inner.z
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
        fn x(&self) -> f64 {
            self.x
        }

        #[pygetset]
        fn y(&self) -> f64 {
            self.y
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
        fn __getattr__(&self, name: PyStrRef, vm: &VirtualMachine) -> PyResult<f64> {
            let name = name.to_str().unwrap_or_default();
            matrix_index(name)
                .map(|index| self.inner.borrow().m[index])
                .ok_or_else(|| attribute_error(name, vm))
        }

        #[pymethod]
        fn __setattr__(&self, name: PyStrRef, value: f64, vm: &VirtualMachine) -> PyResult<()> {
            let name = name.to_str().unwrap_or_default();
            let index = matrix_index(name).ok_or_else(|| attribute_error(name, vm))?;
            self.inner.borrow_mut().m[index] = value;
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
        fn Angle(&self) -> f64 {
            self.inner.angle()
        }

        #[pygetset]
        fn Axis(&self) -> PyVector {
            PyVector {
                inner: self.inner.axis(),
            }
        }

        #[pymethod]
        fn multVec(&self, vector: PyRef<PyVector>) -> PyVector {
            PyVector {
                inner: self.inner.to_matrix().transform(&vector.inner),
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
        fn Base(&self) -> PyVector {
            PyVector {
                inner: self.inner.base,
            }
        }

        #[pygetset]
        fn Rotation(&self) -> PyRotation {
            PyRotation {
                inner: self.inner.rotation,
            }
        }

        #[pymethod]
        fn multVec(&self, vector: PyRef<PyVector>) -> PyVector {
            PyVector {
                inner: self.inner.to_matrix().transform(&vector.inner),
            }
        }

        #[pymethod]
        fn inverse(&self) -> Self {
            Self {
                inner: self.inner.inverse(),
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
        fn __str__(&self) -> String {
            self.name.clone()
        }
    }
}
