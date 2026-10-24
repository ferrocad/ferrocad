//! The Python proxies: `FormProxy`, `WidgetProxy` and `SignalProxy`.
//!
//! These are intentionally *not* a Qt object model. They expose the measured
//! panel contract from `docs/workbenches/qt-strategy.md` §2.4 over the state in
//! [`crate::state`]:
//!
//! * a form looks widgets up by name and supports the root operations;
//! * a widget exposes the typed properties its class actually has;
//! * a signal is a callback list that the host (today: tests, later: gpui)
//!   drives with `emit`.
//!
//! A member outside that set raises `AttributeError` naming the form, class,
//! widget and member. Nothing returns a silent `None`.

use std::cell::RefCell;
use std::rc::Rc;

use pyo3::exceptions::{PyAttributeError, PyIndexError, PyTypeError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyString, PyTuple};
use pyo3::IntoPyObjectExt;

use crate::state::{
    is_dialog_root, is_signal, Callbacks, FormState, WidgetState,
};

// ---------------------------------------------------------------------------
// Shared dispatch
// ---------------------------------------------------------------------------

/// Call every callback connected to `signal`.
///
/// A Python exception is caught at this boundary, printed once and swallowed:
/// one broken panel handler must not kill the form. The callback list is
/// snapshotted first so a handler may reconnect or disconnect safely.
pub(crate) fn dispatch(
    py: Python<'_>,
    state: &Rc<RefCell<FormState>>,
    widget: usize,
    signal: &str,
    callbacks: &Callbacks,
    args: &Bound<'_, PyTuple>,
) {
    let snapshot: Vec<Py<PyAny>> = callbacks
        .borrow()
        .iter()
        .map(|callback| callback.clone_ref(py))
        .collect();
    for callback in snapshot {
        if let Err(error) = callback.bind(py).call(args, None) {
            let (class, name) = {
                let state = state.borrow();
                let widget = &state.widgets[widget];
                (widget.class.clone(), widget.name.clone())
            };
            eprintln!(
                "ferrocad_ui_py: {} '{}' signal '{}' handler raised: {}",
                class, name, signal, error
            );
        }
    }
}

/// Emit `signal` only when someone connected to it.
///
/// Setters call this after changing state, mirroring Qt's `setText` →
/// `textChanged` and friends without allocating signal tables for unused forms.
fn emit_if_connected(
    py: Python<'_>,
    state: &Rc<RefCell<FormState>>,
    widget: usize,
    signal: &str,
    args: &Bound<'_, PyTuple>,
) {
    let callbacks = {
        let state = state.borrow();
        state.widgets[widget].signals.get(signal).cloned()
    };
    if let Some(callbacks) = callbacks {
        dispatch(py, state, widget, signal, &callbacks, args);
    }
}

// ---------------------------------------------------------------------------
// SignalProxy
// ---------------------------------------------------------------------------

/// A connectable signal on a widget.
#[pyclass(module = "ferrocad_ui_py", name = "SignalProxy", unsendable)]
pub struct SignalProxy {
    pub(crate) state: Rc<RefCell<FormState>>,
    pub(crate) widget: usize,
    pub(crate) signal: String,
    pub(crate) callbacks: Callbacks,
}

#[pymethods]
impl SignalProxy {
    /// Connect `callable`; it is invoked with whatever `emit` is given.
    fn connect(&self, callable: &Bound<'_, PyAny>) -> PyResult<()> {
        if !callable.is_callable() {
            return Err(PyTypeError::new_err(
                "SignalProxy.connect() expects a callable",
            ));
        }
        self.callbacks.borrow_mut().push(callable.clone().unbind());
        Ok(())
    }

    /// Disconnect one callable (by identity), or all callables when called with
    /// no argument. Returns whether anything was removed.
    #[pyo3(signature = (callable=None))]
    fn disconnect(&self, py: Python<'_>, callable: Option<&Bound<'_, PyAny>>) -> bool {
        let mut callbacks = self.callbacks.borrow_mut();
        let before = callbacks.len();
        match callable {
            Some(callable) => callbacks.retain(|stored| !stored.bind(py).is(callable)),
            None => callbacks.clear(),
        }
        before != callbacks.len()
    }

    /// Call every connected callback. Exceptions are reported and swallowed.
    #[pyo3(signature = (*args))]
    fn emit(&self, py: Python<'_>, args: &Bound<'_, PyTuple>) -> PyResult<()> {
        dispatch(py, &self.state, self.widget, &self.signal, &self.callbacks, args);
        Ok(())
    }

    /// The number of connected callbacks.
    fn __len__(&self) -> usize {
        self.callbacks.borrow().len()
    }

    fn __repr__(&self) -> String {
        let state = self.state.borrow();
        let widget = &state.widgets[self.widget];
        format!(
            "SignalProxy({}.{}, connected={})",
            widget.name,
            self.signal,
            self.callbacks.borrow().len()
        )
    }
}

// ---------------------------------------------------------------------------
// WidgetProxy
// ---------------------------------------------------------------------------

/// A widget in a loaded form.
#[pyclass(module = "ferrocad_ui_py", name = "WidgetProxy", unsendable)]
pub struct WidgetProxy {
    pub(crate) state: Rc<RefCell<FormState>>,
    pub(crate) widget: usize,
}

impl WidgetProxy {
    fn error(&self, member: &str) -> PyErr {
        self.state.borrow().error(self.widget, member)
    }

    fn supported(&self, check: fn(&WidgetState) -> bool) -> bool {
        let state = self.state.borrow();
        check(&state.widgets[self.widget])
    }

    fn proxy(&self, py: Python<'_>, widget: usize) -> PyResult<Py<WidgetProxy>> {
        Py::new(
            py,
            WidgetProxy {
                state: self.state.clone(),
                widget,
            },
        )
    }
}

#[pymethods]
impl WidgetProxy {
    // -- text -------------------------------------------------------------

    /// The widget's `text` property.
    fn text(&self) -> PyResult<String> {
        let state = self.state.borrow();
        let widget = &state.widgets[self.widget];
        if !widget.supports_text {
            return Err(state.error(self.widget, "text"));
        }
        Ok(widget.text.clone().unwrap_or_default())
    }

    /// Set the widget's `text` property and emit `textChanged`.
    fn setText(&self, py: Python<'_>, text: String) -> PyResult<()> {
        if !self.supported(|widget| widget.supports_text) {
            return Err(self.error("setText"));
        }
        {
            let mut state = self.state.borrow_mut();
            state.widgets[self.widget].text = Some(text.clone());
        }
        let args = PyTuple::new(py, [text.as_str()])?;
        emit_if_connected(py, &self.state, self.widget, "textChanged", &args);
        Ok(())
    }

    // -- title (QGroupBox) -------------------------------------------------

    /// The `QGroupBox` title.
    fn title(&self) -> PyResult<String> {
        let state = self.state.borrow();
        let widget = &state.widgets[self.widget];
        match &widget.title {
            Some(title) => Ok(title.clone()),
            None => Err(state.error(self.widget, "title")),
        }
    }

    /// Set the `QGroupBox` title.
    fn setTitle(&self, title: String) -> PyResult<()> {
        if !self.supported(|widget| widget.title.is_some()) {
            return Err(self.error("setTitle"));
        }
        let mut state = self.state.borrow_mut();
        state.widgets[self.widget].title = Some(title);
        Ok(())
    }

    // -- numeric value -----------------------------------------------------

    /// The numeric value (`QSpinBox`, `QDoubleSpinBox`, `Gui::QuantitySpinBox`).
    fn value(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let (value, is_int) = {
            let state = self.state.borrow();
            let widget = &state.widgets[self.widget];
            if !widget.supports_value {
                return Err(state.error(self.widget, "value"));
            }
            (widget.value, widget.value_is_int)
        };
        if is_int {
            (value as i64).into_py_any(py)
        } else {
            value.into_py_any(py)
        }
    }

    /// Set the numeric value and emit `valueChanged`.
    fn setValue(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        if !self.supported(|widget| widget.supports_value) {
            return Err(self.error("setValue"));
        }
        let raw = value.extract::<f64>().map_err(|_| {
            PyTypeError::new_err("setValue() expects an int or float")
        })?;
        let (stored, is_int) = {
            let mut state = self.state.borrow_mut();
            let widget = &mut state.widgets[self.widget];
            let stored = if widget.value_is_int {
                raw.trunc()
            } else {
                raw
            };
            widget.value = stored;
            (stored, widget.value_is_int)
        };
        if is_int {
            let args = PyTuple::new(py, [stored as i64])?;
            emit_if_connected(py, &self.state, self.widget, "valueChanged", &args);
        } else {
            let args = PyTuple::new(py, [stored])?;
            emit_if_connected(py, &self.state, self.widget, "valueChanged", &args);
        }
        Ok(())
    }

    // -- checked -----------------------------------------------------------

    /// Whether the widget is checked.
    fn isChecked(&self) -> PyResult<bool> {
        let state = self.state.borrow();
        let widget = &state.widgets[self.widget];
        if !widget.supports_checked {
            return Err(state.error(self.widget, "isChecked"));
        }
        Ok(widget.checked)
    }

    /// Set the checked state and emit `stateChanged`, `toggled` and
    /// `checkStateChanged`.
    fn setChecked(&self, py: Python<'_>, checked: bool) -> PyResult<()> {
        if !self.supported(|widget| widget.supports_checked) {
            return Err(self.error("setChecked"));
        }
        {
            let mut state = self.state.borrow_mut();
            state.widgets[self.widget].checked = checked;
        }
        let state_args = PyTuple::new(py, [checked])?;
        emit_if_connected(
            py,
            &self.state,
            self.widget,
            "stateChanged",
            &state_args,
        );
        emit_if_connected(py, &self.state, self.widget, "toggled", &state_args);
        // Qt's `Qt::CheckState`: Unchecked = 0, Checked = 2.
        let check_state = PyTuple::new(py, [if checked { 2_i64 } else { 0_i64 }])?;
        emit_if_connected(
            py,
            &self.state,
            self.widget,
            "checkStateChanged",
            &check_state,
        );
        Ok(())
    }

    // -- combo box ---------------------------------------------------------

    /// The current combo-box index (`-1` when empty).
    fn currentIndex(&self) -> PyResult<i64> {
        let state = self.state.borrow();
        let widget = &state.widgets[self.widget];
        if !widget.supports_combo {
            return Err(state.error(self.widget, "currentIndex"));
        }
        Ok(widget.current_index)
    }

    /// Select an index and emit `currentIndexChanged`.
    fn setCurrentIndex(&self, py: Python<'_>, index: i64) -> PyResult<()> {
        let (supported, count) = {
            let state = self.state.borrow();
            let widget = &state.widgets[self.widget];
            (widget.supports_combo, widget.items.len())
        };
        if !supported {
            return Err(self.error("setCurrentIndex"));
        }
        if index < -1 || (index >= 0 && index as usize >= count) {
            return Err(PyIndexError::new_err(format!(
                "setCurrentIndex({index}) is out of range for {count} item(s)"
            )));
        }
        {
            let mut state = self.state.borrow_mut();
            let widget = &mut state.widgets[self.widget];
            widget.current_index = index;
            if index >= 0 {
                widget.current_text.clear();
            }
        }
        let args = PyTuple::new(py, [index])?;
        emit_if_connected(
            py,
            &self.state,
            self.widget,
            "currentIndexChanged",
            &args,
        );
        Ok(())
    }

    /// The text of the current combo-box item.
    fn currentText(&self) -> PyResult<String> {
        let state = self.state.borrow();
        let widget = &state.widgets[self.widget];
        if !widget.supports_combo {
            return Err(state.error(self.widget, "currentText"));
        }
        if widget.current_index >= 0 {
            if let Some(text) = widget.items.get(widget.current_index as usize) {
                return Ok(text.clone());
            }
        }
        Ok(widget.current_text.clone())
    }

    // -- enabled / visible -------------------------------------------------

    fn isEnabled(&self) -> bool {
        self.state.borrow().widgets[self.widget].enabled
    }

    fn setEnabled(&self, enabled: bool) {
        let mut state = self.state.borrow_mut();
        state.widgets[self.widget].enabled = enabled;
    }

    fn isVisible(&self) -> bool {
        self.state.borrow().widgets[self.widget].visible
    }

    fn setVisible(&self, visible: bool) {
        let mut state = self.state.borrow_mut();
        state.widgets[self.widget].visible = visible;
    }

    /// No-op visibility helper (there is no window in Q2a).
    fn show(&self) {
        self.setVisible(true);
    }

    /// No-op visibility helper (there is no window in Q2a).
    fn hide(&self) {
        self.setVisible(false);
    }

    // -- identity and tree -------------------------------------------------

    /// The Qt object name (`objectName()` in Qt).
    fn objectName(&self) -> String {
        self.state.borrow().widgets[self.widget].name.clone()
    }

    /// The Qt class name, for example `QSpinBox`.
    fn className(&self) -> String {
        self.state.borrow().widgets[self.widget].class.clone()
    }

    /// Direct child widgets, in document order.
    fn children(&self, py: Python<'_>) -> PyResult<Vec<Py<WidgetProxy>>> {
        let children = {
            let state = self.state.borrow();
            state.widgets[self.widget].children.clone()
        };
        children
            .into_iter()
            .map(|child| self.proxy(py, child))
            .collect()
    }

    /// Find a descendant by widget name, class-name string or class object.
    ///
    /// The optional second argument follows Qt: `findChild(cls, "name")`
    /// resolves by name and treats the class as advisory.
    #[pyo3(signature = (query, name=None))]
    fn findChild(
        &self,
        py: Python<'_>,
        query: &Bound<'_, PyAny>,
        name: Option<String>,
    ) -> PyResult<Option<Py<WidgetProxy>>> {
        let found = {
            let state = self.state.borrow();
            find_descendant(&state, self.widget, query, name.as_deref())?
        };
        found.map(|widget| self.proxy(py, widget)).transpose()
    }

    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
        if !name.starts_with("__") && is_signal(name) {
            let callbacks = {
                let mut state = self.state.borrow_mut();
                state.callbacks(self.widget, name)
            };
            let signal = Py::new(
                py,
                SignalProxy {
                    state: self.state.clone(),
                    widget: self.widget,
                    signal: name.to_owned(),
                    callbacks,
                },
            )?;
            return Ok(signal.into_any());
        }
        Err(self.error(name))
    }

    fn __repr__(&self) -> String {
        let state = self.state.borrow();
        let widget = &state.widgets[self.widget];
        format!("WidgetProxy({} '{}')", widget.class, widget.name)
    }
}

// ---------------------------------------------------------------------------
// FormProxy
// ---------------------------------------------------------------------------

/// A loaded form (the value `PySideUic.loadUi` returns).
#[pyclass(module = "ferrocad_ui_py", name = "FormProxy", unsendable)]
pub struct FormProxy {
    pub(crate) state: Rc<RefCell<FormState>>,
}

impl FormProxy {
    pub(crate) fn new(state: FormState) -> Self {
        Self {
            state: Rc::new(RefCell::new(state)),
        }
    }

    fn proxy(&self, py: Python<'_>, widget: usize) -> PyResult<Py<WidgetProxy>> {
        Py::new(
            py,
            WidgetProxy {
                state: self.state.clone(),
                widget,
            },
        )
    }
}

#[pymethods]
impl FormProxy {
    /// Widget lookup by Qt object name.
    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
        if name.starts_with("__") {
            let state = self.state.borrow();
            return Err(PyAttributeError::new_err(format!(
                "form '{}' ({}) has no attribute '{}'",
                state.class, state.file, name
            )));
        }
        let widget = {
            let state = self.state.borrow();
            match state.by_name.get(name) {
                Some(widget) => *widget,
                None => {
                    let names: Vec<&str> = state
                        .widgets
                        .iter()
                        .map(|widget| widget.name.as_str())
                        .collect();
                    return Err(PyAttributeError::new_err(format!(
                        "form '{}' ({}) has no widget '{}'; available widgets: {}",
                        state.class,
                        state.file,
                        name,
                        names.join(", ")
                    )));
                }
            }
        };
        Ok(self.proxy(py, widget)?.into_any())
    }

    /// Find a widget by name, class-name string or class object.
    #[pyo3(signature = (query, name=None))]
    fn findChild(
        &self,
        py: Python<'_>,
        query: &Bound<'_, PyAny>,
        name: Option<String>,
    ) -> PyResult<Option<Py<WidgetProxy>>> {
        let found = {
            let state = self.state.borrow();
            find_in_form(&state, query, name.as_deref())?
        };
        found.map(|widget| self.proxy(py, widget)).transpose()
    }

    /// The form's window title (`windowTitle` in the `.ui`).
    fn windowTitle(&self) -> String {
        self.state.borrow().window_title.clone()
    }

    fn setWindowTitle(&self, title: String) {
        self.state.borrow_mut().window_title = title;
    }

    /// No-op: Q2a has no window.
    fn show(&self) {}

    /// No-op: Q2a has no window.
    fn hide(&self) {}

    /// No-op: Q2a has no window or event loop.
    fn close(&self) {}

    /// Accept a dialog. Only `QDialog` roots have this operation.
    fn accept(&self) -> PyResult<()> {
        let state = self.state.borrow();
        let root_class = &state.widgets[state.root].class;
        if !is_dialog_root(root_class) {
            return Err(PyAttributeError::new_err(format!(
                "form '{}' ({}) is a {}; accept() is only available on QDialog roots",
                state.class, state.file, root_class
            )));
        }
        Ok(())
    }

    /// Reject a dialog. Only `QDialog` roots have this operation.
    fn reject(&self) -> PyResult<()> {
        let state = self.state.borrow();
        let root_class = &state.widgets[state.root].class;
        if !is_dialog_root(root_class) {
            return Err(PyAttributeError::new_err(format!(
                "form '{}' ({}) is a {}; reject() is only available on QDialog roots",
                state.class, state.file, root_class
            )));
        }
        Ok(())
    }

    /// The root widget's Qt object name.
    fn objectName(&self) -> String {
        let state = self.state.borrow();
        state.widgets[state.root].name.clone()
    }

    /// The form's `<class>` name.
    fn className(&self) -> String {
        self.state.borrow().class.clone()
    }

    /// The root widget's direct children.
    fn children(&self, py: Python<'_>) -> PyResult<Vec<Py<WidgetProxy>>> {
        let children = {
            let state = self.state.borrow();
            state.widgets[state.root].children.clone()
        };
        children
            .into_iter()
            .map(|child| self.proxy(py, child))
            .collect()
    }

    fn __len__(&self) -> usize {
        self.state.borrow().widgets.len()
    }

    fn __repr__(&self) -> String {
        let state = self.state.borrow();
        format!(
            "FormProxy({} from {}, {} widgets)",
            state.class,
            state.file,
            state.widgets.len()
        )
    }
}

// ---------------------------------------------------------------------------
// findChild resolution
// ---------------------------------------------------------------------------

enum Query {
    /// A widget name first, then a class name.
    Text(String),
    /// A class name only.
    Class(String),
}

fn query_kind(query: &Bound<'_, PyAny>) -> PyResult<Query> {
    if let Ok(text) = query.downcast::<PyString>() {
        return Ok(Query::Text(text.to_str()?.to_owned()));
    }
    if let Ok(name) = query.getattr("__name__") {
        if let Ok(name) = name.extract::<String>() {
            return Ok(Query::Class(name));
        }
    }
    Err(PyTypeError::new_err(
        "findChild() expects a widget name, a class-name string or a Qt class",
    ))
}

fn matches(state: &FormState, widget: usize, query: &Query) -> bool {
    let widget = &state.widgets[widget];
    match query {
        Query::Text(text) => &widget.name == text || &widget.class == text,
        Query::Class(class) => &widget.class == class,
    }
}

fn find_in_form(
    state: &FormState,
    query: &Bound<'_, PyAny>,
    name: Option<&str>,
) -> PyResult<Option<usize>> {
    if let Some(name) = name {
        return Ok(state.widgets.iter().position(|widget| widget.name == name));
    }
    let query = query_kind(query)?;
    // A name wins over a class when the string could be either.
    if let Query::Text(text) = &query {
        if let Some(widget) = state.widgets.iter().position(|widget| &widget.name == text) {
            return Ok(Some(widget));
        }
    }
    Ok((0..state.widgets.len()).find(|&index| matches(state, index, &query)))
}

fn find_descendant(
    state: &FormState,
    start: usize,
    query: &Bound<'_, PyAny>,
    name: Option<&str>,
) -> PyResult<Option<usize>> {
    let order = descendants(state, start);
    if let Some(name) = name {
        return Ok(order
            .into_iter()
            .find(|widget| state.widgets[*widget].name == name));
    }
    let query = query_kind(query)?;
    if let Query::Text(text) = &query {
        if let Some(widget) = order
            .iter()
            .copied()
            .find(|widget| &state.widgets[*widget].name == text)
        {
            return Ok(Some(widget));
        }
    }
    Ok(order
        .into_iter()
        .find(|widget| matches(state, *widget, &query)))
}

/// Descendant widget indices in pre-order, excluding `start`.
fn descendants(state: &FormState, start: usize) -> Vec<usize> {
    let mut order = Vec::new();
    let mut stack: Vec<usize> = state.widgets[start].children.iter().rev().copied().collect();
    while let Some(widget) = stack.pop() {
        order.push(widget);
        for child in state.widgets[widget].children.iter().rev() {
            stack.push(*child);
        }
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyo3::exceptions::PyAttributeError;
    use pyo3::types::PyList;

    const UI: &str = r#"
<ui version="4.0">
 <class>TestForm</class>
 <widget class="QWidget" name="Form">
  <layout class="QVBoxLayout" name="layout">
   <item><widget class="QLabel" name="label">
    <property name="text"><string>Hello</string></property>
   </widget></item>
   <item><widget class="QSpinBox" name="spin">
    <property name="value"><number>5</number></property>
   </widget></item>
   <item><widget class="QCheckBox" name="check"/></item>
   <item><widget class="QComboBox" name="combo">
    <item><property name="text"><string>One</string></property></item>
    <item><property name="text"><string>Two</string></property></item>
   </widget></item>
   <item><widget class="QGroupBox" name="group">
    <property name="title"><string>Group</string></property>
    <layout class="QVBoxLayout" name="nested">
     <item><widget class="QLabel" name="nested_label"/></item>
    </layout>
   </widget></item>
  </layout>
 </widget>
</ui>
"#;

    fn form(py: Python<'_>) -> Bound<'_, PyAny> {
        let form = ferrocad_ui::Form::parse_str(UI).expect("inline form parses");
        let state = crate::state::build_state(&form, "inline.ui");
        Py::new(py, FormProxy::new(state))
            .expect("proxy allocates")
            .into_bound(py)
            .into_any()
    }

    #[test]
    fn form_lookup_properties_and_signals_round_trip() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let form = form(py);

            let label = form.getattr("label").unwrap();
            assert_eq!(label.call_method0("text").unwrap().extract::<String>().unwrap(), "Hello");
            label.call_method1("setText", ("Changed",)).unwrap();
            assert_eq!(label.call_method0("text").unwrap().extract::<String>().unwrap(), "Changed");

            let spin = form.getattr("spin").unwrap();
            assert_eq!(spin.call_method0("value").unwrap().extract::<i64>().unwrap(), 5);
            spin.call_method1("setValue", (8,)).unwrap();
            assert_eq!(spin.call_method0("value").unwrap().extract::<i64>().unwrap(), 8);

            let check = form.getattr("check").unwrap();
            assert!(!check.call_method0("isChecked").unwrap().extract::<bool>().unwrap());
            check.call_method1("setChecked", (true,)).unwrap();
            assert!(check.call_method0("isChecked").unwrap().extract::<bool>().unwrap());

            let combo = form.getattr("combo").unwrap();
            assert_eq!(combo.call_method0("currentText").unwrap().extract::<String>().unwrap(), "One");
            combo.call_method1("setCurrentIndex", (1,)).unwrap();
            assert_eq!(combo.call_method0("currentIndex").unwrap().extract::<i64>().unwrap(), 1);
            assert_eq!(combo.call_method0("currentText").unwrap().extract::<String>().unwrap(), "Two");

            let calls = PyList::empty(py);
            let append = calls.getattr("append").unwrap();
            let changed = spin.getattr("valueChanged").unwrap();
            changed.call_method1("connect", (append,)).unwrap();
            spin.call_method1("setValue", (9,)).unwrap();
            changed.call_method1("emit", (11,)).unwrap();
            assert_eq!(calls.len(), 2);
            assert_eq!(calls.get_item(0).unwrap().extract::<i64>().unwrap(), 9);
            assert_eq!(calls.get_item(1).unwrap().extract::<i64>().unwrap(), 11);
        });
    }

    #[test]
    fn find_child_and_tree_operations() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let form = form(py);
            let by_name = form
                .call_method1("findChild", ("nested_label",))
                .unwrap();
            assert_eq!(
                by_name.call_method0("objectName").unwrap().extract::<String>().unwrap(),
                "nested_label"
            );
            let by_class = form.call_method1("findChild", ("QSpinBox",)).unwrap();
            assert_eq!(
                by_class.call_method0("objectName").unwrap().extract::<String>().unwrap(),
                "spin"
            );
            let group = form.getattr("group").unwrap();
            let nested = group.call_method1("findChild", ("nested_label",)).unwrap();
            assert_eq!(
                nested.call_method0("objectName").unwrap().extract::<String>().unwrap(),
                "nested_label"
            );
            assert_eq!(form.call_method0("children").unwrap().len().unwrap(), 5);
        });
    }

    #[test]
    fn unsupported_members_raise_attribute_error() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let form = form(py);
            let spin = form.getattr("spin").unwrap();
            let error = spin.call_method0("text").unwrap_err();
            assert!(error.is_instance_of::<PyAttributeError>(py));
            let message = error.to_string();
            assert!(message.contains("QSpinBox"), "{message}");
            assert!(message.contains("'spin'"), "{message}");
            assert!(message.contains("'text'"), "{message}");

            let error = form.getattr("missing").unwrap_err();
            assert!(error.is_instance_of::<PyAttributeError>(py));
            let message = error.to_string();
            assert!(message.contains("missing"), "{message}");
        });
    }

    #[test]
    fn accept_is_only_on_dialog_roots() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let form = form(py);
            let error = form.call_method0("accept").unwrap_err();
            assert!(error.is_instance_of::<PyAttributeError>(py));
        });
    }
}
