//! The Python proxies: `FormProxy`, `WidgetProxy`, `LayoutProxy`,
//! `LayoutItemProxy` and `SignalProxy`.
//!
//! These are intentionally *not* a Qt object model. They expose the measured
//! panel contract from `docs/workbenches/qt-strategy.md` §2.4 over the state in
//! [`crate::state`]:
//!
//! * a form looks widgets up by name and supports the root operations;
//! * a widget exposes the typed properties its class actually has;
//! * a layout is an ordered item list that can be rearranged without Qt;
//! * a signal is a callback list that the host (today: tests, later: gpui)
//!   drives with `emit`.
//!
//! A member outside that set raises `AttributeError` naming the form, class,
//! widget and member. Nothing returns a silent `None` for an unsupported member.

use std::cell::RefCell;
use std::rc::Rc;

use pyo3::exceptions::{PyAttributeError, PyIndexError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyString, PyTuple};
use pyo3::IntoPyObjectExt;

use crate::state::{
    clamp, is_dialog_root, is_grid_layout, is_signal, Callbacks, FormState, LayoutEntry,
    LayoutEntryKind, LayoutState, WidgetState,
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

/// Report an out-of-range layout or combo index.
fn index_error(kind: &str, index: i64, count: usize) -> PyErr {
    PyIndexError::new_err(format!(
        "{kind}({index}) is out of range for {count} item(s)"
    ))
}

/// Return a number as `int` for `QSpinBox` and `float` for the double variants.
fn number_any(py: Python<'_>, value: f64, is_int: bool) -> PyResult<Py<PyAny>> {
    if is_int {
        (value as i64).into_py_any(py)
    } else {
        value.into_py_any(py)
    }
}

/// Parse a colour from a tuple/list of 3 or 4 ints, a `#rrggbb`/`#aarrggbb`
/// string, or an object with `getRgb()`/`getRgbF()`/`rgba()`.
fn parse_color(value: &Bound<'_, PyAny>) -> PyResult<(u8, u8, u8, u8)> {
    fn component(value: f64) -> PyResult<u8> {
        if !(0.0..=255.0).contains(&value) {
            return Err(PyValueError::new_err(format!(
                "colour component {value} is outside 0..=255"
            )));
        }
        Ok(value.round() as u8)
    }

    fn from_floats(values: &[f64]) -> PyResult<(u8, u8, u8, u8)> {
        if values.len() < 3 {
            return Err(PyValueError::new_err(
                "a colour needs at least three components",
            ));
        }
        let alpha = values.get(3).copied().unwrap_or(255.0);
        Ok((
            component(values[0])?,
            component(values[1])?,
            component(values[2])?,
            component(alpha)?,
        ))
    }

    if let Ok(text) = value.downcast::<PyString>() {
        let text = text.to_str()?.trim().trim_start_matches('#');
        let digits: Vec<u8> = text
            .chars()
            .map(|ch| ch.to_digit(16).map(|digit| digit as u8))
            .collect::<Option<Vec<u8>>>()
            .ok_or_else(|| PyValueError::new_err(format!("invalid colour string {text:?}")))?;
        let byte = |index: usize| -> u8 { digits[index] * 16 + digits[index + 1] };
        return match digits.len() {
            6 => Ok((byte(0), byte(2), byte(4), 255)),
            // Qt's eight-digit form is #AARRGGBB.
            8 => Ok((byte(2), byte(4), byte(6), byte(0))),
            _ => Err(PyValueError::new_err(format!(
                "colour string {text:?} must have 6 or 8 hex digits"
            ))),
        };
    }

    if value.hasattr("getRgb")? {
        let rgb = value.call_method0("getRgb")?;
        return from_floats(&rgb.extract::<Vec<f64>>()?);
    }
    if value.hasattr("getRgbF")? {
        let rgb = value.call_method0("getRgbF")?;
        let floats = rgb.extract::<Vec<f64>>()?;
        let scaled: Vec<f64> = floats.iter().map(|part| part * 255.0).collect();
        return from_floats(&scaled);
    }
    if value.hasattr("rgba")? {
        let rgba = value.call_method0("rgba")?.extract::<i64>()?;
        let rgba = rgba as u64;
        return Ok((
            ((rgba >> 16) & 0xff) as u8,
            ((rgba >> 8) & 0xff) as u8,
            (rgba & 0xff) as u8,
            ((rgba >> 24) & 0xff) as u8,
        ));
    }
    if let Ok(values) = value.extract::<Vec<f64>>() {
        return from_floats(&values);
    }
    Err(PyTypeError::new_err(
        "setProperty('color') expects an (r, g, b[, a]) sequence, a hex string \
         or a QColor-like object",
    ))
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

    fn property_error(&self, name: &str) -> PyErr {
        let state = self.state.borrow();
        let widget = &state.widgets[self.widget];
        PyAttributeError::new_err(format!(
            "{} '{}' has no property '{}' (form {})",
            widget.class, widget.name, name, state.file
        ))
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

    fn layout_proxy(&self, py: Python<'_>, layout: usize) -> PyResult<Py<LayoutProxy>> {
        Py::new(
            py,
            LayoutProxy {
                state: self.state.clone(),
                layout,
            },
        )
    }

    fn found_proxy(&self, py: Python<'_>, found: Found) -> PyResult<Py<PyAny>> {
        match found {
            Found::Widget(widget) => Ok(self.proxy(py, widget)?.into_any()),
            Found::Layout(layout) => Ok(self.layout_proxy(py, layout)?.into_any()),
        }
    }

    fn range_bound(&self, py: Python<'_>, minimum: bool) -> PyResult<Py<PyAny>> {
        let state = self.state.borrow();
        let widget = &state.widgets[self.widget];
        if !widget.supports_range {
            return Err(state.error(
                self.widget,
                if minimum { "minimum" } else { "maximum" },
            ));
        }
        let value = if minimum {
            widget.minimum
        } else {
            widget.maximum
        }
        .unwrap_or(0.0);
        number_any(py, value, widget.value_is_int)
    }

    fn set_range_bound(
        &self,
        py: Python<'_>,
        value: &Bound<'_, PyAny>,
        minimum: bool,
    ) -> PyResult<()> {
        if !self.supported(|widget| widget.supports_range) {
            return Err(self.error(if minimum { "setMinimum" } else { "setMaximum" }));
        }
        let raw = value.extract::<f64>().map_err(|_| {
            PyTypeError::new_err("setMinimum()/setMaximum() expect an int or float")
        })?;
        let (stored, is_int, changed) = {
            let mut state = self.state.borrow_mut();
            let widget = &mut state.widgets[self.widget];
            let bound = if widget.value_is_int {
                raw.trunc()
            } else {
                raw
            };
            if minimum {
                widget.minimum = Some(bound);
                if widget.maximum.is_some_and(|maximum| bound > maximum) {
                    widget.maximum = Some(bound);
                }
            } else {
                widget.maximum = Some(bound);
                if widget.minimum.is_some_and(|minimum| bound < minimum) {
                    widget.minimum = Some(bound);
                }
            }
            let clamped = clamp(widget.value, widget.minimum, widget.maximum);
            let changed = clamped != widget.value;
            widget.value = clamped;
            (clamped, widget.value_is_int, changed)
        };
        if changed {
            let args = if is_int {
                PyTuple::new(py, [stored as i64])?
            } else {
                PyTuple::new(py, [stored])?
            };
            emit_if_connected(py, &self.state, self.widget, "valueChanged", &args);
        }
        Ok(())
    }

    fn add_items(&self, py: Python<'_>, texts: Vec<String>) -> PyResult<()> {
        if !self.supported(|widget| widget.supports_combo) {
            return Err(self.error("addItem"));
        }
        let changed = {
            let mut state = self.state.borrow_mut();
            let widget = &mut state.widgets[self.widget];
            let became_current = widget.items.is_empty() && !texts.is_empty();
            widget.items.extend(texts);
            if became_current {
                widget.current_index = 0;
                widget.current_text.clear();
            }
            became_current
        };
        if changed {
            let args = PyTuple::new(py, [0_i64])?;
            emit_if_connected(
                py,
                &self.state,
                self.widget,
                "currentIndexChanged",
                &args,
            );
        }
        Ok(())
    }

    fn apply_current_index(&self, py: Python<'_>, index: i64) -> PyResult<()> {
        let count = {
            let state = self.state.borrow();
            let widget = &state.widgets[self.widget];
            if !widget.supports_combo {
                return Err(state.error(self.widget, "setCurrentIndex"));
            }
            widget.items.len()
        };
        if index < -1 || (index >= 0 && index as usize >= count) {
            return Err(index_error("setCurrentIndex", index, count));
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

    fn store_file_name(&self, py: Python<'_>, name: String) -> PyResult<()> {
        if !self.supported(|widget| widget.supports_file_name) {
            return Err(self.error("setFileName"));
        }
        {
            let mut state = self.state.borrow_mut();
            state.widgets[self.widget].file_name = name.clone();
        }
        let args = PyTuple::new(py, [name.as_str()])?;
        emit_if_connected(py, &self.state, self.widget, "fileNameChanged", &args);
        Ok(())
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
        number_any(py, value, is_int)
    }

    /// Set the numeric value, clamped to `minimum`/`maximum`, and emit
    /// `valueChanged`.
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
            let mut stored = if widget.value_is_int {
                raw.trunc()
            } else {
                raw
            };
            stored = clamp(stored, widget.minimum, widget.maximum);
            widget.value = stored;
            (stored, widget.value_is_int)
        };
        let args = if is_int {
            PyTuple::new(py, [stored as i64])?
        } else {
            PyTuple::new(py, [stored])?
        };
        emit_if_connected(py, &self.state, self.widget, "valueChanged", &args);
        Ok(())
    }

    // -- range -------------------------------------------------------------

    /// The inclusive minimum, for the classes that have a numeric range.
    fn minimum(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.range_bound(py, true)
    }

    /// The inclusive maximum, for the classes that have a numeric range.
    fn maximum(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.range_bound(py, false)
    }

    /// Set the minimum, re-clamping the value, and emit `valueChanged` when it
    /// moved.
    fn setMinimum(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.set_range_bound(py, value, true)
    }

    /// Set the maximum, re-clamping the value, and emit `valueChanged` when it
    /// moved.
    fn setMaximum(&self, py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<()> {
        self.set_range_bound(py, value, false)
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
        self.apply_current_index(py, index)
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

    /// The number of combo-box items.
    fn count(&self) -> PyResult<usize> {
        let state = self.state.borrow();
        let widget = &state.widgets[self.widget];
        if !widget.supports_combo {
            return Err(state.error(self.widget, "count"));
        }
        Ok(widget.items.len())
    }

    /// The text of item `index`.
    fn itemText(&self, index: i64) -> PyResult<String> {
        let state = self.state.borrow();
        let widget = &state.widgets[self.widget];
        if !widget.supports_combo {
            return Err(state.error(self.widget, "itemText"));
        }
        if index < 0 || index as usize >= widget.items.len() {
            return Err(index_error("itemText", index, widget.items.len()));
        }
        Ok(widget.items[index as usize].clone())
    }

    /// Append one item; the first item of an empty box becomes current.
    fn addItem(&self, py: Python<'_>, text: String) -> PyResult<()> {
        self.add_items(py, vec![text])
    }

    /// Append several items; the first item of an empty box becomes current.
    fn addItems(&self, py: Python<'_>, texts: Vec<String>) -> PyResult<()> {
        self.add_items(py, texts)
    }

    /// Select the item with `text`, or set the edit text on an editable box.
    /// A non-editable box ignores an unknown text, as in Qt.
    fn setCurrentText(&self, py: Python<'_>, text: String) -> PyResult<()> {
        let (supported, found, editable) = {
            let state = self.state.borrow();
            let widget = &state.widgets[self.widget];
            (
                widget.supports_combo,
                widget.items.iter().position(|item| item == &text),
                widget.editable,
            )
        };
        if !supported {
            return Err(self.error("setCurrentText"));
        }
        if let Some(index) = found {
            return self.apply_current_index(py, index as i64);
        }
        if !editable {
            return Ok(());
        }
        let changed = {
            let mut state = self.state.borrow_mut();
            let widget = &mut state.widgets[self.widget];
            let changed = widget.current_index != -1 || widget.current_text != text;
            widget.current_index = -1;
            widget.current_text = text;
            changed
        };
        if changed {
            let args = PyTuple::new(py, [-1_i64])?;
            emit_if_connected(
                py,
                &self.state,
                self.widget,
                "currentIndexChanged",
                &args,
            );
        }
        Ok(())
    }

    /// Remove every item and reset the current index to `-1`.
    fn clear(&self, py: Python<'_>) -> PyResult<()> {
        let changed = {
            let mut state = self.state.borrow_mut();
            let widget = &mut state.widgets[self.widget];
            if !widget.supports_combo {
                return Err(state.error(self.widget, "clear"));
            }
            let changed = widget.current_index != -1;
            widget.items.clear();
            widget.current_index = -1;
            widget.current_text.clear();
            changed
        };
        if changed {
            let args = PyTuple::new(py, [-1_i64])?;
            emit_if_connected(
                py,
                &self.state,
                self.widget,
                "currentIndexChanged",
                &args,
            );
        }
        Ok(())
    }

    // -- dynamic properties ------------------------------------------------

    /// Read `rawValue`, `color`, `fileName`, `minimum` or `maximum`.
    fn property(&self, py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
        let state = self.state.borrow();
        let widget = &state.widgets[self.widget];
        match name {
            "rawValue" if widget.supports_raw_value => {
                // Quantity spin boxes share the numeric value.
                let value = if widget.supports_value {
                    widget.value
                } else {
                    widget.raw_value
                };
                value.into_py_any(py)
            }
            "color" if widget.supports_color => {
                let (red, green, blue, alpha) = widget.color.unwrap_or((0, 0, 0, 255));
                PyTuple::new(py, [red, green, blue, alpha])?.into_py_any(py)
            }
            "fileName" if widget.supports_file_name => widget.file_name.clone().into_py_any(py),
            "minimum" if widget.supports_range => {
                let value = widget.minimum.unwrap_or(0.0);
                number_any(py, value, widget.value_is_int)
            }
            "maximum" if widget.supports_range => {
                let value = widget.maximum.unwrap_or(0.0);
                number_any(py, value, widget.value_is_int)
            }
            _ => Err(self.property_error(name)),
        }
    }

    /// Write one of the supported dynamic properties.
    fn setProperty(
        &self,
        py: Python<'_>,
        name: &str,
        value: &Bound<'_, PyAny>,
    ) -> PyResult<()> {
        match name {
            "rawValue" => {
                if !self.supported(|widget| widget.supports_raw_value) {
                    return Err(self.property_error(name));
                }
                let raw = value.extract::<f64>().map_err(|_| {
                    PyTypeError::new_err("setProperty('rawValue') expects an int or float")
                })?;
                if self.supported(|widget| widget.supports_value) {
                    // Keep value()/minimum()/maximum() consistent.
                    return self.setValue(py, value);
                }
                {
                    let mut state = self.state.borrow_mut();
                    state.widgets[self.widget].raw_value = raw;
                }
                let args = PyTuple::new(py, [raw])?;
                emit_if_connected(py, &self.state, self.widget, "valueChanged", &args);
                Ok(())
            }
            "color" => {
                if !self.supported(|widget| widget.supports_color) {
                    return Err(self.property_error(name));
                }
                let color = parse_color(value)?;
                let mut state = self.state.borrow_mut();
                state.widgets[self.widget].color = Some(color);
                Ok(())
            }
            "fileName" => {
                if !self.supported(|widget| widget.supports_file_name) {
                    return Err(self.property_error(name));
                }
                let name = value.extract::<String>().map_err(|_| {
                    PyTypeError::new_err("setProperty('fileName') expects a string")
                })?;
                self.store_file_name(py, name)
            }
            "minimum" => self.set_range_bound(py, value, true),
            "maximum" => self.set_range_bound(py, value, false),
            _ => Err(self.property_error(name)),
        }
    }

    /// The `Gui::FileChooser` file name.
    fn fileName(&self) -> PyResult<String> {
        let state = self.state.borrow();
        let widget = &state.widgets[self.widget];
        if !widget.supports_file_name {
            return Err(state.error(self.widget, "fileName"));
        }
        Ok(widget.file_name.clone())
    }

    /// Set the `Gui::FileChooser` file name and emit `fileNameChanged`.
    fn setFileName(&self, py: Python<'_>, name: String) -> PyResult<()> {
        self.store_file_name(py, name)
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

    /// No-op visibility helper (there is no window in Q2b).
    fn hide(&self) {
        self.setVisible(false);
    }

    // -- layout and parent ------------------------------------------------

    /// The layout this widget owns (`None` when it has none), as in Qt.
    fn layout(&self, py: Python<'_>) -> PyResult<Option<Py<LayoutProxy>>> {
        let layout = self.state.borrow().widgets[self.widget].layout;
        layout
            .map(|layout| self.layout_proxy(py, layout))
            .transpose()
    }

    /// Reparent this widget within the form. `setParent(None)` makes it a
    /// top-level widget in the model. Layout membership is deliberately left
    /// alone: Qt's `setParent` does not remove the widget from a layout.
    #[pyo3(signature = (parent=None))]
    fn setParent(
        &self,
        parent: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<()> {
        let target = match parent {
            None => None,
            Some(object) => {
                let reference = object.extract::<PyRef<WidgetProxy>>().map_err(|_| {
                    PyTypeError::new_err("setParent() expects a WidgetProxy or None")
                })?;
                if !Rc::ptr_eq(&reference.state, &self.state) {
                    return Err(PyValueError::new_err(
                        "setParent() cannot move a widget between forms",
                    ));
                }
                Some(reference.widget)
            }
        };
        let mut state = self.state.borrow_mut();
        if let Some(target) = target {
            if target == self.widget || descendants(&state, self.widget).contains(&target) {
                return Err(PyValueError::new_err(
                    "setParent() cannot create a cycle in the widget tree",
                ));
            }
        }
        if let Some(old_parent) = state.widgets[self.widget].parent.take() {
            state.widgets[old_parent]
                .children
                .retain(|child| *child != self.widget);
        }
        if let Some(target) = target {
            if !state.widgets[target].children.contains(&self.widget) {
                state.widgets[target].children.push(self.widget);
            }
            state.widgets[self.widget].parent = Some(target);
        }
        Ok(())
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

    /// Find a descendant widget or layout by name, class-name string or class
    /// object.
    ///
    /// The optional second argument follows Qt: `findChild(cls, "name")`
    /// resolves by name and treats the class as advisory.
    #[pyo3(signature = (query, name=None))]
    fn findChild(
        &self,
        py: Python<'_>,
        query: &Bound<'_, PyAny>,
        name: Option<String>,
    ) -> PyResult<Option<Py<PyAny>>> {
        let found = {
            let state = self.state.borrow();
            find_descendant(&state, self.widget, query, name.as_deref())?
        };
        found
            .map(|found| self.found_proxy(py, found))
            .transpose()
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

    fn layout_proxy(&self, py: Python<'_>, layout: usize) -> PyResult<Py<LayoutProxy>> {
        Py::new(
            py,
            LayoutProxy {
                state: self.state.clone(),
                layout,
            },
        )
    }

    fn found_proxy(&self, py: Python<'_>, found: Found) -> PyResult<Py<PyAny>> {
        match found {
            Found::Widget(widget) => Ok(self.proxy(py, widget)?.into_any()),
            Found::Layout(layout) => Ok(self.layout_proxy(py, layout)?.into_any()),
        }
    }

    fn check_dialog(&self, operation: &str) -> PyResult<usize> {
        let state = self.state.borrow();
        let root_class = &state.widgets[state.root].class;
        if !is_dialog_root(root_class) {
            return Err(PyAttributeError::new_err(format!(
                "form '{}' ({}) is a {}; {} is only available on QDialog roots",
                state.class, state.file, root_class, operation
            )));
        }
        Ok(state.root)
    }

    fn finish_dialog(&self, py: Python<'_>, accepted: bool) -> PyResult<()> {
        let root = self.check_dialog(if accepted { "accept()" } else { "reject()" })?;
        self.state.borrow_mut().result = if accepted { 1 } else { 0 };
        let signal = if accepted { "accepted" } else { "rejected" };
        let args = PyTuple::empty(py);
        emit_if_connected(py, &self.state, root, signal, &args);
        Ok(())
    }
}

#[pymethods]
impl FormProxy {
    /// Widget lookup by Qt object name. A `QDialog` form also exposes the
    /// form-level `accepted`/`rejected` signals.
    fn __getattr__(&self, py: Python<'_>, name: &str) -> PyResult<Py<PyAny>> {
        if name.starts_with("__") {
            let state = self.state.borrow();
            return Err(PyAttributeError::new_err(format!(
                "form '{}' ({}) has no attribute '{}'",
                state.class, state.file, name
            )));
        }
        let (root, dialog) = {
            let state = self.state.borrow();
            (
                state.root,
                is_dialog_root(&state.widgets[state.root].class),
            )
        };
        if dialog && matches!(name, "accepted" | "rejected") {
            let callbacks = {
                let mut state = self.state.borrow_mut();
                state.callbacks(root, name)
            };
            let signal = Py::new(
                py,
                SignalProxy {
                    state: self.state.clone(),
                    widget: root,
                    signal: name.to_owned(),
                    callbacks,
                },
            )?;
            return Ok(signal.into_any());
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

    /// Find a widget or layout by name, class-name string or class object.
    #[pyo3(signature = (query, name=None))]
    fn findChild(
        &self,
        py: Python<'_>,
        query: &Bound<'_, PyAny>,
        name: Option<String>,
    ) -> PyResult<Option<Py<PyAny>>> {
        let found = {
            let state = self.state.borrow();
            find_in_form(&state, query, name.as_deref())?
        };
        found.map(|found| self.found_proxy(py, found)).transpose()
    }

    /// The form's window title (`windowTitle` in the `.ui`).
    fn windowTitle(&self) -> String {
        self.state.borrow().window_title.clone()
    }

    fn setWindowTitle(&self, title: String) {
        self.state.borrow_mut().window_title = title;
    }

    /// No-op: Q2b has no window.
    fn show(&self) {}

    /// No-op: Q2b has no window.
    fn hide(&self) {}

    /// No-op: Q2b has no window or event loop.
    fn close(&self) {}

    /// Accept a dialog: set the result to `QDialog::Accepted` (1) and emit the
    /// form-level `accepted` signal.
    fn accept(&self, py: Python<'_>) -> PyResult<()> {
        self.finish_dialog(py, true)
    }

    /// Reject a dialog: set the result to `QDialog::Rejected` (0) and emit the
    /// form-level `rejected` signal.
    fn reject(&self, py: Python<'_>) -> PyResult<()> {
        self.finish_dialog(py, false)
    }

    /// Return the stored dialog result without running an event loop.
    ///
    /// Q2b is not a modal host: `exec_()` is non-blocking and simply reports
    /// what `accept()`/`reject()` (or the `QDialog::Rejected` default) left in
    /// `result()`. Q3's gpui modal replaces it with the shell's async dialog.
    fn exec_(&self) -> PyResult<i64> {
        self.check_dialog("exec_()")?;
        Ok(self.state.borrow().result)
    }

    /// The stored dialog result: 1 after `accept()`, 0 otherwise.
    fn result(&self) -> PyResult<i64> {
        self.check_dialog("result()")?;
        Ok(self.state.borrow().result)
    }

    /// The layout installed on the root widget (`None` when it has none).
    fn layout(&self, py: Python<'_>) -> PyResult<Option<Py<LayoutProxy>>> {
        let layout = {
            let state = self.state.borrow();
            state.widgets[state.root].layout
        };
        layout
            .map(|layout| self.layout_proxy(py, layout))
            .transpose()
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
// LayoutProxy / LayoutItemProxy
// ---------------------------------------------------------------------------

/// A layout in a loaded form.
///
/// The model is deliberately small: an ordered item list plus grid positions.
/// There is no geometry, no stretch and no rendering; `addWidget`/`takeAt` move
/// widgets between the layout and the widget tree so an upstream panel can
/// rebuild rows without Qt.
#[pyclass(module = "ferrocad_ui_py", name = "LayoutProxy", unsendable)]
pub struct LayoutProxy {
    pub(crate) state: Rc<RefCell<FormState>>,
    pub(crate) layout: usize,
}

impl LayoutProxy {
    fn error(&self, member: &str) -> PyErr {
        self.state.borrow().layout_error(self.layout, member)
    }

    fn entry(&self, index: i64) -> PyResult<LayoutEntry> {
        let state = self.state.borrow();
        let layout = &state.layouts[self.layout];
        if index < 0 || index as usize >= layout.items.len() {
            return Err(index_error("layout item", index, layout.items.len()));
        }
        Ok(layout.items[index as usize].clone())
    }
}

#[pymethods]
impl LayoutProxy {
    /// The number of items in the layout.
    fn count(&self) -> usize {
        self.state.borrow().layouts[self.layout].items.len()
    }

    /// The item at `index`, or `None` when the index is out of range.
    fn itemAt(&self, py: Python<'_>, index: i64) -> PyResult<Option<Py<LayoutItemProxy>>> {
        let count = self.state.borrow().layouts[self.layout].items.len();
        if index < 0 || index as usize >= count {
            return Ok(None);
        }
        let entry = self.entry(index)?;
        Ok(Some(Py::new(
            py,
            LayoutItemProxy {
                state: self.state.clone(),
                entry,
            },
        )?))
    }

    /// Remove and return the item at `index`. The widget/layout itself is not
    /// deleted and keeps its parent; Qt's `takeAt` behaves the same way.
    fn takeAt(&self, py: Python<'_>, index: i64) -> PyResult<Py<LayoutItemProxy>> {
        let entry = {
            let mut state = self.state.borrow_mut();
            let count = state.layouts[self.layout].items.len();
            if index < 0 || index as usize >= count {
                return Err(index_error("takeAt", index, count));
            }
            let entry = state.layouts[self.layout].items.remove(index as usize);
            if let LayoutEntryKind::Widget(widget) = entry.kind {
                if state.widgets[widget].owning_layout == Some(self.layout) {
                    state.widgets[widget].owning_layout = None;
                }
            }
            entry
        };
        Py::new(
            py,
            LayoutItemProxy {
                state: self.state.clone(),
                entry,
            },
        )
    }

    /// Append a widget. A grid layout accepts `row`/`column`/`rowspan`/
    /// `colspan`; other layouts only accept `addWidget(widget)`.
    ///
    /// As in Qt, adding a widget reparents it to the layout's widget and
    /// removes it from any layout it was already in.
    #[pyo3(signature = (widget, row=None, column=None, rowspan=1, colspan=1))]
    fn addWidget(
        &self,
        widget: &Bound<'_, PyAny>,
        row: Option<u32>,
        column: Option<u32>,
        rowspan: u32,
        colspan: u32,
    ) -> PyResult<()> {
        let (index, same_form) = {
            let reference = widget.extract::<PyRef<WidgetProxy>>().map_err(|_| {
                PyTypeError::new_err("addWidget() expects a WidgetProxy")
            })?;
            (reference.widget, Rc::ptr_eq(&reference.state, &self.state))
        };
        if !same_form {
            return Err(PyValueError::new_err(
                "addWidget() cannot move a widget between forms",
            ));
        }

        let mut state = self.state.borrow_mut();
        let class = state.layouts[self.layout].class.clone();
        if !is_grid_layout(&class)
            && (row.is_some() || column.is_some() || rowspan != 1 || colspan != 1)
        {
            return Err(PyValueError::new_err(format!(
                "{class} only supports addWidget(widget) without row/column/spans"
            )));
        }
        let parent = state.layouts[self.layout].parent;
        if let Some(parent) = parent {
            if parent == index || descendants(&state, index).contains(&parent) {
                return Err(PyValueError::new_err(
                    "addWidget() cannot create a cycle in the widget tree",
                ));
            }
        }

        // A widget has exactly one layout; adding it moves it.
        if let Some(old) = state.widgets[index].owning_layout.take() {
            state.layouts[old]
                .items
                .retain(|entry| !matches!(entry.kind, LayoutEntryKind::Widget(w) if w == index));
        }
        if let Some(old_parent) = state.widgets[index].parent.take() {
            state.widgets[old_parent]
                .children
                .retain(|child| *child != index);
        }
        if let Some(parent) = parent {
            if !state.widgets[parent].children.contains(&index) {
                state.widgets[parent].children.push(index);
            }
            state.widgets[index].parent = Some(parent);
        }
        state.layouts[self.layout].items.push(LayoutEntry {
            kind: LayoutEntryKind::Widget(index),
            row,
            column,
            row_span: rowspan,
            column_span: colspan,
        });
        state.widgets[index].owning_layout = Some(self.layout);
        Ok(())
    }

    /// Append a nested layout, with the same grid arguments as `addWidget`.
    #[pyo3(signature = (layout, row=None, column=None, rowspan=1, colspan=1))]
    fn addLayout(
        &self,
        layout: &Bound<'_, PyAny>,
        row: Option<u32>,
        column: Option<u32>,
        rowspan: u32,
        colspan: u32,
    ) -> PyResult<()> {
        let (child, same_form) = {
            let reference = layout.extract::<PyRef<LayoutProxy>>().map_err(|_| {
                PyTypeError::new_err("addLayout() expects a LayoutProxy")
            })?;
            (reference.layout, Rc::ptr_eq(&reference.state, &self.state))
        };
        if !same_form {
            return Err(PyValueError::new_err(
                "addLayout() cannot move a layout between forms",
            ));
        }
        if child == self.layout {
            return Err(PyValueError::new_err(
                "addLayout() cannot add a layout to itself",
            ));
        }

        let mut state = self.state.borrow_mut();
        let class = state.layouts[self.layout].class.clone();
        if !is_grid_layout(&class)
            && (row.is_some() || column.is_some() || rowspan != 1 || colspan != 1)
        {
            return Err(PyValueError::new_err(format!(
                "{class} only supports addLayout(layout) without row/column/spans"
            )));
        }
        if layout_contains(&state, child, self.layout) {
            return Err(PyValueError::new_err(
                "addLayout() cannot create a cycle in the layout tree",
            ));
        }
        for candidate in &mut state.layouts {
            candidate
                .items
                .retain(|entry| !matches!(entry.kind, LayoutEntryKind::Layout(id) if id == child));
        }
        state.layouts[child].parent = state.layouts[self.layout].parent;
        state.layouts[self.layout].items.push(LayoutEntry {
            kind: LayoutEntryKind::Layout(child),
            row,
            column,
            row_span: rowspan,
            column_span: colspan,
        });
        Ok(())
    }

    /// The layout's Qt object name (empty when unnamed).
    fn objectName(&self) -> String {
        self.state.borrow().layouts[self.layout]
            .name
            .clone()
            .unwrap_or_default()
    }

    /// The layout's Qt class name, for example `QGridLayout`.
    fn className(&self) -> String {
        self.state.borrow().layouts[self.layout].class.clone()
    }

    fn __getattr__(&self, name: &str) -> PyResult<Py<PyAny>> {
        Err(self.error(name))
    }

    fn __repr__(&self) -> String {
        let state = self.state.borrow();
        let layout = &state.layouts[self.layout];
        format!(
            "LayoutProxy({} '{}', {} items)",
            layout.class,
            layout.name.as_deref().unwrap_or("<unnamed>"),
            layout.items.len()
        )
    }
}

/// The value `LayoutProxy.takeAt()`/`itemAt()` returns: a detached view of one
/// layout item.
#[pyclass(module = "ferrocad_ui_py", name = "LayoutItemProxy", unsendable)]
pub struct LayoutItemProxy {
    pub(crate) state: Rc<RefCell<FormState>>,
    pub(crate) entry: LayoutEntry,
}

#[pymethods]
impl LayoutItemProxy {
    /// The widget held by the item, or `None` for a nested layout/spacer.
    fn widget(&self, py: Python<'_>) -> PyResult<Option<Py<WidgetProxy>>> {
        match self.entry.kind {
            LayoutEntryKind::Widget(widget) => Ok(Some(Py::new(
                py,
                WidgetProxy {
                    state: self.state.clone(),
                    widget,
                },
            )?)),
            _ => Ok(None),
        }
    }

    /// The nested layout held by the item, or `None` for a widget/spacer.
    fn layout(&self, py: Python<'_>) -> PyResult<Option<Py<LayoutProxy>>> {
        match self.entry.kind {
            LayoutEntryKind::Layout(layout) => Ok(Some(Py::new(
                py,
                LayoutProxy {
                    state: self.state.clone(),
                    layout,
                },
            )?)),
            _ => Ok(None),
        }
    }

    /// The grid row the item was declared/added at, if any.
    fn row(&self) -> Option<u32> {
        self.entry.row
    }

    /// The grid column the item was declared/added at, if any.
    fn column(&self) -> Option<u32> {
        self.entry.column
    }

    /// The row span (1 unless explicitly given).
    fn rowSpan(&self) -> u32 {
        self.entry.row_span
    }

    /// The column span (1 unless explicitly given).
    fn colSpan(&self) -> u32 {
        self.entry.column_span
    }

    fn __getattr__(&self, name: &str) -> PyResult<Py<PyAny>> {
        let state = self.state.borrow();
        Err(PyAttributeError::new_err(format!(
            "LayoutItem has no attribute '{}' (form {})",
            name, state.file
        )))
    }

    fn __repr__(&self) -> String {
        match self.entry.kind {
            LayoutEntryKind::Widget(widget) => {
                let state = self.state.borrow();
                format!("LayoutItemProxy(widget '{}')", state.widgets[widget].name)
            }
            LayoutEntryKind::Layout(layout) => {
                let state = self.state.borrow();
                format!(
                    "LayoutItemProxy(layout '{}')",
                    state.layouts[layout].name.as_deref().unwrap_or("<unnamed>")
                )
            }
            LayoutEntryKind::Spacer(spacer) => format!("LayoutItemProxy(spacer #{spacer})"),
        }
    }
}

/// Whether `container` or any nested layout below it is `target`.
fn layout_contains(state: &FormState, container: usize, target: usize) -> bool {
    if container == target {
        return true;
    }
    let mut stack = vec![container];
    while let Some(layout) = stack.pop() {
        for entry in &state.layouts[layout].items {
            if let LayoutEntryKind::Layout(child) = entry.kind {
                if child == target {
                    return true;
                }
                stack.push(child);
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// findChild resolution
// ---------------------------------------------------------------------------

enum Query {
    /// A widget/layout name first, then a class name.
    Text(String),
    /// A class name only.
    Class(String),
}

/// What `findChild` resolved to.
#[derive(Clone, Copy)]
enum Found {
    Widget(usize),
    Layout(usize),
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
        "findChild() expects a name, a class-name string or a Qt class",
    ))
}

fn widget_matches(state: &FormState, widget: usize, query: &Query) -> bool {
    let widget = &state.widgets[widget];
    match query {
        Query::Text(text) => &widget.name == text || &widget.class == text,
        Query::Class(class) => &widget.class == class,
    }
}

fn layout_matches(layout: &LayoutState, query: &Query) -> bool {
    match query {
        Query::Text(text) => layout.name.as_deref() == Some(text) || &layout.class == text,
        Query::Class(class) => &layout.class == class,
    }
}

/// First layout matching `name` or `query` inside `start` and its descendants.
///
/// `None` searches the whole form; `Some(widget)` includes layouts owned by
/// that widget (Qt treats an installed layout as a QObject child).
fn find_layout(
    state: &FormState,
    start: Option<usize>,
    name: Option<&str>,
    query: Option<&Query>,
) -> Option<usize> {
    let scope = start.map(|start| {
        let mut scope = vec![start];
        scope.extend(descendants(state, start));
        scope
    });
    (0..state.layouts.len()).find(|&index| {
        let layout = &state.layouts[index];
        let in_scope = match (&scope, layout.parent) {
            (None, _) => true,
            (Some(scope), Some(parent)) => scope.contains(&parent),
            (Some(_), None) => false,
        };
        if !in_scope {
            return false;
        }
        match (name, query) {
            (Some(name), _) => layout.name.as_deref() == Some(name),
            (None, Some(query)) => layout_matches(layout, query),
            (None, None) => false,
        }
    })
}

fn find_in_form(
    state: &FormState,
    query: &Bound<'_, PyAny>,
    name: Option<&str>,
) -> PyResult<Option<Found>> {
    if let Some(name) = name {
        if let Some(widget) = state.widgets.iter().position(|widget| widget.name == name) {
            return Ok(Some(Found::Widget(widget)));
        }
        return Ok(find_layout(state, None, Some(name), None).map(Found::Layout));
    }
    let query = query_kind(query)?;
    // A name wins over a class when the string could be either.
    if let Query::Text(text) = &query {
        if let Some(widget) = state.widgets.iter().position(|widget| &widget.name == text) {
            return Ok(Some(Found::Widget(widget)));
        }
        if let Some(layout) = find_layout(state, None, Some(text), None) {
            return Ok(Some(Found::Layout(layout)));
        }
    }
    if let Some(widget) = (0..state.widgets.len()).find(|&index| widget_matches(state, index, &query)) {
        return Ok(Some(Found::Widget(widget)));
    }
    Ok(find_layout(state, None, None, Some(&query)).map(Found::Layout))
}

fn find_descendant(
    state: &FormState,
    start: usize,
    query: &Bound<'_, PyAny>,
    name: Option<&str>,
) -> PyResult<Option<Found>> {
    let order = descendants(state, start);
    if let Some(name) = name {
        if let Some(widget) = order
            .iter()
            .copied()
            .find(|widget| state.widgets[*widget].name == name)
        {
            return Ok(Some(Found::Widget(widget)));
        }
        return Ok(find_layout(state, Some(start), Some(name), None).map(Found::Layout));
    }
    let query = query_kind(query)?;
    if let Query::Text(text) = &query {
        if let Some(widget) = order
            .iter()
            .copied()
            .find(|widget| &state.widgets[*widget].name == text)
        {
            return Ok(Some(Found::Widget(widget)));
        }
        if let Some(layout) = find_layout(state, Some(start), Some(text), None) {
            return Ok(Some(Found::Layout(layout)));
        }
    }
    if let Some(widget) = order
        .into_iter()
        .find(|widget| widget_matches(state, *widget, &query))
    {
        return Ok(Some(Found::Widget(widget)));
    }
    Ok(find_layout(state, Some(start), None, Some(&query)).map(Found::Layout))
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
    use pyo3::types::{PyDict, PyList};

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

    const PROPS_UI: &str = r#"
<ui version="4.0">
 <class>PropsForm</class>
 <widget class="QWidget" name="Form">
  <layout class="QGridLayout" name="grid">
   <item row="0" column="0"><widget class="QSpinBox" name="spin">
    <property name="minimum"><number>1</number></property>
    <property name="maximum"><number>10</number></property>
    <property name="value"><number>5</number></property>
   </widget></item>
   <item row="0" column="1"><widget class="Gui::InputField" name="field">
    <property name="quantity"><double>2.5</double></property>
   </widget></item>
   <item row="1" column="0"><widget class="Gui::ColorButton" name="color">
    <property name="color"><color><red>10</red><green>20</green><blue>30</blue></color></property>
   </widget></item>
   <item row="1" column="1"><widget class="Gui::FileChooser" name="file"/></item>
   <item row="2" column="0"><widget class="QComboBox" name="combo"/></item>
  </layout>
 </widget>
</ui>
"#;

    const DIALOG_UI: &str = r#"
<ui version="4.0">
 <class>DialogForm</class>
 <widget class="QDialog" name="Dialog"/>
</ui>
"#;

    fn form_from<'py>(py: Python<'py>, source: &str) -> Bound<'py, PyAny> {
        let form = ferrocad_ui::Form::parse_str(source).expect("inline form parses");
        let state = crate::state::build_state(&form, "inline.ui");
        Py::new(py, FormProxy::new(state))
            .expect("proxy allocates")
            .into_bound(py)
            .into_any()
    }

    #[test]
    fn layout_surgery_moves_widgets_between_layouts() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let form = form(py);
            let root = form.call_method0("layout").unwrap();
            assert_eq!(root.call_method0("count").unwrap().extract::<usize>().unwrap(), 5);
            assert_eq!(
                root.call_method0("className").unwrap().extract::<String>().unwrap(),
                "QVBoxLayout"
            );

            let group = form.getattr("group").unwrap();
            let nested = group.call_method0("layout").unwrap();
            assert_eq!(nested.call_method0("count").unwrap().extract::<usize>().unwrap(), 1);
            let item = nested.call_method1("takeAt", (0,)).unwrap();
            let label = item.call_method0("widget").unwrap();
            assert_eq!(
                label.call_method0("objectName").unwrap().extract::<String>().unwrap(),
                "nested_label"
            );
            assert_eq!(nested.call_method0("count").unwrap().extract::<usize>().unwrap(), 0);

            // Box layouts only accept the coordinate-free form.
            assert!(root.call_method1("addWidget", (label.clone(), 1, 2)).is_err());
            root.call_method1("addWidget", (label,)).unwrap();
            assert_eq!(root.call_method0("count").unwrap().extract::<usize>().unwrap(), 6);

            root.call_method1("addLayout", (nested,)).unwrap();
            assert_eq!(root.call_method0("count").unwrap().extract::<usize>().unwrap(), 7);

            let found = form.call_method1("findChild", ("QVBoxLayout",)).unwrap();
            assert_eq!(
                found.call_method0("className").unwrap().extract::<String>().unwrap(),
                "QVBoxLayout"
            );
        });
    }

    #[test]
    fn combo_mutation_stays_consistent() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let form = form_from(py, PROPS_UI);
            let combo = form.getattr("combo").unwrap();
            assert_eq!(combo.call_method0("count").unwrap().extract::<usize>().unwrap(), 0);
            assert_eq!(combo.call_method0("currentIndex").unwrap().extract::<i64>().unwrap(), -1);

            combo.call_method1("addItem", ("A",)).unwrap();
            assert_eq!(combo.call_method0("currentIndex").unwrap().extract::<i64>().unwrap(), 0);
            assert_eq!(combo.call_method0("currentText").unwrap().extract::<String>().unwrap(), "A");

            combo
                .call_method1("addItems", (vec!["B", "C"],))
                .unwrap();
            combo.call_method1("setCurrentText", ("B",)).unwrap();
            assert_eq!(combo.call_method0("currentIndex").unwrap().extract::<i64>().unwrap(), 1);
            assert_eq!(combo.call_method1("itemText", (2,)).unwrap().extract::<String>().unwrap(), "C");

            // A non-editable box ignores unknown text.
            combo.call_method1("setCurrentText", ("missing",)).unwrap();
            assert_eq!(combo.call_method0("currentText").unwrap().extract::<String>().unwrap(), "B");

            combo.call_method0("clear").unwrap();
            assert_eq!(combo.call_method0("count").unwrap().extract::<usize>().unwrap(), 0);
            assert_eq!(combo.call_method0("currentIndex").unwrap().extract::<i64>().unwrap(), -1);
            assert_eq!(combo.call_method0("currentText").unwrap().extract::<String>().unwrap(), "");
        });
    }

    #[test]
    fn dynamic_properties_and_range_clamping() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let form = form_from(py, PROPS_UI);

            let spin = form.getattr("spin").unwrap();
            assert_eq!(spin.call_method0("value").unwrap().extract::<i64>().unwrap(), 5);
            spin.call_method1("setValue", (0,)).unwrap();
            assert_eq!(spin.call_method0("value").unwrap().extract::<i64>().unwrap(), 1);
            spin.call_method1("setValue", (20,)).unwrap();
            assert_eq!(spin.call_method0("value").unwrap().extract::<i64>().unwrap(), 10);
            spin.call_method1("setProperty", ("maximum", 5)).unwrap();
            assert_eq!(spin.call_method0("value").unwrap().extract::<i64>().unwrap(), 5);
            assert_eq!(
                spin.call_method1("property", ("minimum",)).unwrap().extract::<i64>().unwrap(),
                1
            );

            let field = form.getattr("field").unwrap();
            assert_eq!(
                field.call_method1("property", ("rawValue",)).unwrap().extract::<f64>().unwrap(),
                2.5
            );
            field.call_method1("setProperty", ("rawValue", 7.5)).unwrap();
            assert_eq!(
                field.call_method1("property", ("rawValue",)).unwrap().extract::<f64>().unwrap(),
                7.5
            );

            let color = form.getattr("color").unwrap();
            assert_eq!(
                color
                    .call_method1("property", ("color",))
                    .unwrap()
                    .extract::<(u8, u8, u8, u8)>()
                    .unwrap(),
                (10, 20, 30, 255)
            );
            color
                .call_method1("setProperty", ("color", (1, 2, 3, 4)))
                .unwrap();
            assert_eq!(
                color
                    .call_method1("property", ("color",))
                    .unwrap()
                    .extract::<(u8, u8, u8, u8)>()
                    .unwrap(),
                (1, 2, 3, 4)
            );

            let file = form.getattr("file").unwrap();
            assert_eq!(file.call_method0("fileName").unwrap().extract::<String>().unwrap(), "");
            file.call_method1("setFileName", ("/tmp/font.ttf",)).unwrap();
            assert_eq!(
                file.call_method0("fileName").unwrap().extract::<String>().unwrap(),
                "/tmp/font.ttf"
            );
        });
    }

    #[test]
    fn dialog_accept_reject_sets_the_result_and_emits() {
        pyo3::prepare_freethreaded_python();
        Python::with_gil(|py| {
            let form = form_from(py, DIALOG_UI);
            assert_eq!(form.call_method0("exec_").unwrap().extract::<i64>().unwrap(), 0);
            assert_eq!(form.call_method0("result").unwrap().extract::<i64>().unwrap(), 0);

            let seen = PyList::empty(py);
            let locals = PyDict::new(py);
            locals.set_item("seen", &seen).unwrap();
            let append = py
                .eval(c"lambda: seen.append(1)", Some(&locals), None)
                .unwrap();
            form.getattr("accepted")
                .unwrap()
                .call_method1("connect", (append.clone(),))
                .unwrap();
            form.getattr("rejected")
                .unwrap()
                .call_method1("connect", (append,))
                .unwrap();

            form.call_method0("accept").unwrap();
            assert_eq!(form.call_method0("result").unwrap().extract::<i64>().unwrap(), 1);
            assert_eq!(form.call_method0("exec_").unwrap().extract::<i64>().unwrap(), 1);
            form.call_method0("reject").unwrap();
            assert_eq!(form.call_method0("result").unwrap().extract::<i64>().unwrap(), 0);
            assert_eq!(seen.len(), 2);
        });
    }
}
