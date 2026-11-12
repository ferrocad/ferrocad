//! The `ferrocad_ui_py` module: the typed form tree behind `PySideUic.loadUi`.
//!
//! `python/FreeCADGui` imports this module lazily, so a host that has it turns a
//! workbench's Qt Designer `.ui` panel into a proxy tree instead of an
//! `ImportError`. The *state machine* is [`ferrocad_ui::state`] — the same one the
//! PyO3 binding drives — and only the Python glue is per-VM, exactly as the ledger
//! splits the two script halves.
//!
//! The surface is deliberately small, and it is the CPython binding's surface: a
//! form looks widgets up by name, `findChild` resolves names and Qt classes, a
//! widget exposes the members its class actually has, signals are connection
//! lists, and `exec_()` reports the stored dialog result without an event loop.
//! A member outside the modelled set raises `AttributeError` naming the form, the
//! class, the widget and the member — there is no silent `None`.

use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use ferrocad_ui::Form;
use ferrocad_ui::state::{
    FormState, Found, LayoutEntry, LayoutEntryKind, Query, clamp, class_query, descendants,
    find_descendant, find_in_form, is_dialog_root, is_grid_layout, is_signal, layout_contains,
    text_query,
};
use rustpython_vm::builtins::{PyBaseExceptionRef, PyBytes, PyStr, PyStrRef, PyValueError};
use rustpython_vm::class::StaticType;
use rustpython_vm::function::{FuncArgs, OptionalArg};
use rustpython_vm::protocol::PySequenceMethods;
use rustpython_vm::types::{AsSequence, Constructor, GetAttr, Representable};
use rustpython_vm::{
    AsObject, Py, PyObjectRef, PyPayload, PyRef, PyResult, VirtualMachine, pyclass, pyexception,
    pymodule,
};

#[pymodule]
mod ferrocad_ui_py {
    #![allow(non_snake_case, non_upper_case_globals)]

    use super::*;

    /// The connected callbacks of one signal.
    type Callbacks = Rc<RefCell<Vec<PyObjectRef>>>;

    /// One loaded form's mutable state, shared by every proxy of that form.
    type State = Rc<RefCell<FormState<PyObjectRef>>>;

    /// A colour as the proxies store it.
    type Color = (u8, u8, u8, u8);

    // ---------------------------------------------------------------------------
    // Shared helpers
    // ---------------------------------------------------------------------------

    /// The four-part `AttributeError`: form, class, widget and member.
    fn widget_error(
        state: &State,
        widget: usize,
        member: &str,
        vm: &VirtualMachine,
    ) -> PyBaseExceptionRef {
        vm.new_attribute_error(state.borrow().widget_message(widget, member))
    }

    /// The same for a `property()` name the widget class does not model.
    fn property_error(
        state: &State,
        widget: usize,
        name: &str,
        vm: &VirtualMachine,
    ) -> PyBaseExceptionRef {
        let state = state.borrow();
        let widget = &state.widgets[widget];
        vm.new_attribute_error(format!(
            "{} '{}' has no property '{}' (form {})",
            widget.class, widget.name, name, state.file
        ))
    }

    fn index_error(
        kind: &str,
        index: i64,
        count: usize,
        vm: &VirtualMachine,
    ) -> PyBaseExceptionRef {
        vm.new_index_error(format!(
            "{kind}({index}) is out of range for {count} item(s)"
        ))
    }

    fn emit_signal(
        state: &State,
        widget: usize,
        signal: &str,
        callbacks: &Callbacks,
        args: Vec<PyObjectRef>,
        vm: &VirtualMachine,
    ) {
        // Snapshot first: a handler may reconnect or disconnect safely.
        let snapshot: Vec<PyObjectRef> = callbacks.borrow().clone();
        for callback in snapshot {
            if let Err(error) = callback.call(args.clone(), vm) {
                let (class, name) = {
                    let state = state.borrow();
                    let widget = &state.widgets[widget];
                    (widget.class.clone(), widget.name.clone())
                };
                eprintln!("ferrocad_ui_py: {class} '{name}' signal '{signal}' handler raised");
                vm.print_exception(&error);
            }
        }
    }

    /// Emit `signal` only when someone connected to it, so an unused form allocates
    /// no signal tables.
    fn emit_if_connected(
        state: &State,
        widget: usize,
        signal: &str,
        args: Vec<PyObjectRef>,
        vm: &VirtualMachine,
    ) {
        let callbacks = {
            let state = state.borrow();
            state.widgets[widget].signals.get(signal).cloned()
        };
        if let Some(callbacks) = callbacks {
            emit_signal(state, widget, signal, &callbacks, args, vm);
        }
    }

    /// A one-argument signal payload.
    fn one_arg(value: PyObjectRef) -> Vec<PyObjectRef> {
        vec![value]
    }

    /// Return a number as `int` for `QSpinBox` and `float` for the double variants.
    fn number_any(value: f64, is_int: bool, vm: &VirtualMachine) -> PyObjectRef {
        if is_int {
            vm.new_pyobj(value as i64)
        } else {
            vm.new_pyobj(value)
        }
    }

    /// An argument that must be an int or a float.
    fn number_arg(value: &PyObjectRef, what: &str, vm: &VirtualMachine) -> PyResult<f64> {
        value
            .try_float(vm)
            .map(|number| number.to_f64())
            .map_err(|_| vm.new_type_error(format!("{what} expects an int or float")))
    }

    fn widget_py(state: &State, widget: usize, vm: &VirtualMachine) -> PyObjectRef {
        PyUiWidget {
            state: state.clone(),
            widget,
        }
        .into_ref(&vm.ctx)
        .into()
    }

    fn layout_py(state: &State, layout: usize, vm: &VirtualMachine) -> PyObjectRef {
        PyUiLayout {
            state: state.clone(),
            layout,
        }
        .into_ref(&vm.ctx)
        .into()
    }

    fn signal_py(
        state: &State,
        widget: usize,
        signal: &str,
        callbacks: Callbacks,
        vm: &VirtualMachine,
    ) -> PyObjectRef {
        PyUiSignal {
            state: state.clone(),
            widget,
            signal: signal.to_owned(),
            callbacks,
        }
        .into_ref(&vm.ctx)
        .into()
    }

    fn found_py(state: &State, found: Found, vm: &VirtualMachine) -> PyObjectRef {
        match found {
            Found::Widget(widget) => widget_py(state, widget, vm),
            Found::Layout(layout) => layout_py(state, layout, vm),
        }
    }

    fn attribute_missing(state: &State, name: &str, vm: &VirtualMachine) -> PyBaseExceptionRef {
        let state = state.borrow();
        vm.new_attribute_error(format!(
            "form '{}' ({}) has no attribute '{}'",
            state.class, state.file, name
        ))
    }

    /// Parse a colour from a tuple/list of 3 or 4 numbers, a `#rrggbb`/`#aarrggbb`
    /// string, or an object with `getRgb()`/`getRgbF()`/`rgba()`.
    fn parse_color(value: &PyObjectRef, vm: &VirtualMachine) -> PyResult<Color> {
        fn component(value: f64, vm: &VirtualMachine) -> PyResult<u8> {
            if !(0.0..=255.0).contains(&value) {
                return Err(
                    vm.new_value_error(format!("colour component {value} is outside 0..=255"))
                );
            }
            Ok(value.round() as u8)
        }

        fn from_floats(values: &[f64], vm: &VirtualMachine) -> PyResult<Color> {
            if values.len() < 3 {
                return Err(vm.new_value_error("a colour needs at least three components"));
            }
            let alpha = values.get(3).copied().unwrap_or(255.0);
            Ok((
                component(values[0], vm)?,
                component(values[1], vm)?,
                component(values[2], vm)?,
                component(alpha, vm)?,
            ))
        }

        if let Some(text) = value.downcast_ref::<PyStr>() {
            let text = text
                .to_str()
                .unwrap_or_default()
                .trim()
                .trim_start_matches('#')
                .to_string();
            let digits: Vec<u8> = text
                .chars()
                .map(|ch| ch.to_digit(16).map(|digit| digit as u8))
                .collect::<Option<Vec<u8>>>()
                .ok_or_else(|| vm.new_value_error(format!("invalid colour string {text:?}")))?;
            let byte = |index: usize| -> u8 { digits[index] * 16 + digits[index + 1] };
            return match digits.len() {
                6 => Ok((byte(0), byte(2), byte(4), 255)),
                // Qt's eight-digit form is #AARRGGBB.
                8 => Ok((byte(2), byte(4), byte(6), byte(0))),
                _ => Err(vm.new_value_error(format!(
                    "colour string {text:?} must have 6 or 8 hex digits"
                ))),
            };
        }

        for method in ["getRgb", "getRgbF"] {
            if let Ok(getter) = value.get_attr(vm.ctx.intern_str(method), vm) {
                let rgb = getter.call((), vm)?;
                let floats: Vec<f64> = rgb.try_into_value::<Vec<f64>>(vm)?;
                let scaled: Vec<f64> = if method == "getRgbF" {
                    floats.iter().map(|part| part * 255.0).collect()
                } else {
                    floats
                };
                return from_floats(&scaled, vm);
            }
        }
        if let Ok(getter) = value.get_attr(vm.ctx.intern_str("rgba"), vm) {
            let rgba: Vec<PyObjectRef> = getter.call((), vm)?.try_into_value(vm)?;
            if let Some(rgba) = rgba
                .first()
                .and_then(|value| value.try_to_value::<i64>(vm).ok())
            {
                let rgba = rgba as u64;
                return Ok((
                    ((rgba >> 16) & 0xff) as u8,
                    ((rgba >> 8) & 0xff) as u8,
                    (rgba & 0xff) as u8,
                    ((rgba >> 24) & 0xff) as u8,
                ));
            }
        }
        if let Ok(values) = value.clone().try_into_value::<Vec<f64>>(vm) {
            return from_floats(&values, vm);
        }
        Err(vm.new_type_error(
            "setProperty('color') expects an (r, g, b[, a]) sequence, a hex string \
             or a QColor-like object",
        ))
    }

    // ---------------------------------------------------------------------------
    // SignalProxy
    // ---------------------------------------------------------------------------

    /// A connectable signal on a widget.
    #[pyattr]
    #[pyclass(name = "SignalProxy", module = "ferrocad_ui_py")]
    #[derive(PyPayload)]
    pub struct PyUiSignal {
        state: State,
        widget: usize,
        signal: String,
        callbacks: Callbacks,
    }

    impl fmt::Debug for PyUiSignal {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "PyUiSignal({})", self.signal)
        }
    }

    impl AsSequence for PyUiSignal {
        fn as_sequence() -> &'static PySequenceMethods {
            static METHODS: PySequenceMethods = PySequenceMethods {
                length: Some(|seq, _vm| {
                    Ok(PyUiSignal::sequence_downcast(seq).callbacks.borrow().len())
                }),
                ..PySequenceMethods::NOT_IMPLEMENTED
            };
            &METHODS
        }
    }

    impl Representable for PyUiSignal {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            let state = zelf.state.borrow();
            let widget = &state.widgets[zelf.widget];
            Ok(format!(
                "SignalProxy({}.{}, connected={})",
                widget.name,
                zelf.signal,
                zelf.callbacks.borrow().len()
            ))
        }
    }

    #[pyclass(with(Representable, AsSequence))]
    impl PyUiSignal {
        /// Connect `callable`; it is invoked with whatever `emit` is given.
        #[pymethod]
        fn connect(zelf: &Py<Self>, callable: PyObjectRef, vm: &VirtualMachine) -> PyResult<()> {
            if !callable.is_callable() {
                return Err(vm.new_type_error("SignalProxy.connect() expects a callable"));
            }
            zelf.callbacks.borrow_mut().push(callable);
            Ok(())
        }

        /// Disconnect one callable (by identity), or all callables when called with
        /// no argument. Returns whether anything was removed.
        #[pymethod]
        fn disconnect(
            zelf: &Py<Self>,
            callable: OptionalArg<PyObjectRef>,
            vm: &VirtualMachine,
        ) -> bool {
            let mut callbacks = zelf.callbacks.borrow_mut();
            let before = callbacks.len();
            match callable.into_option() {
                Some(callable) => callbacks.retain(|stored| !stored.is(&callable)),
                None => callbacks.clear(),
            }
            let _ = vm;
            before != callbacks.len()
        }

        /// Call every connected callback. Exceptions are reported and swallowed.
        #[pymethod]
        fn emit(zelf: &Py<Self>, args: FuncArgs, vm: &VirtualMachine) {
            let args = args.args;
            emit_signal(
                &zelf.state,
                zelf.widget,
                &zelf.signal,
                &zelf.callbacks,
                args,
                vm,
            );
        }
    }

    // ---------------------------------------------------------------------------
    // WidgetProxy
    // ---------------------------------------------------------------------------

    /// A widget in a loaded form.
    #[pyattr]
    #[pyclass(name = "WidgetProxy", module = "ferrocad_ui_py")]
    #[derive(PyPayload)]
    pub struct PyUiWidget {
        state: State,
        widget: usize,
    }

    impl fmt::Debug for PyUiWidget {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "PyUiWidget({})", self.widget)
        }
    }

    impl Representable for PyUiWidget {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            let state = zelf.state.borrow();
            let widget = &state.widgets[zelf.widget];
            Ok(format!("WidgetProxy({} '{}')", widget.class, widget.name))
        }
    }

    impl GetAttr for PyUiWidget {
        fn getattro(zelf: &Py<Self>, name: &Py<PyStr>, vm: &VirtualMachine) -> PyResult {
            let object: PyObjectRef = zelf.to_owned().into();
            if let Ok(value) = object.generic_getattr(name, vm) {
                return Ok(value);
            }
            let name = name.to_str().unwrap_or_default();
            if !name.starts_with("__") && is_signal(name) {
                let callbacks = zelf.state.borrow_mut().callbacks(zelf.widget, name);
                return Ok(signal_py(&zelf.state, zelf.widget, name, callbacks, vm));
            }
            Err(widget_error(&zelf.state, zelf.widget, name, vm))
        }
    }

    impl PyUiWidget {
        fn supported(
            &self,
            check: impl Fn(&ferrocad_ui::state::WidgetState<PyObjectRef>) -> bool,
        ) -> bool {
            let state = self.state.borrow();
            check(&state.widgets[self.widget])
        }

        /// The layout proxy of an owned layout.
        fn owned_layout(&self, vm: &VirtualMachine) -> PyObjectRef {
            match self.state.borrow().widgets[self.widget].layout {
                Some(layout) => layout_py(&self.state, layout, vm),
                None => vm.ctx.none(),
            }
        }

        fn range_bound(&self, minimum: bool, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let state = self.state.borrow();
            let widget = &state.widgets[self.widget];
            if !widget.supports_range {
                return Err(widget_error(
                    &self.state,
                    self.widget,
                    if minimum { "minimum" } else { "maximum" },
                    vm,
                ));
            }
            let value = if minimum {
                widget.minimum
            } else {
                widget.maximum
            }
            .unwrap_or(0.0);
            Ok(number_any(value, widget.value_is_int, vm))
        }

        fn set_range_bound(
            &self,
            value: &PyObjectRef,
            minimum: bool,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            if !self.supported(|widget| widget.supports_range) {
                return Err(widget_error(
                    &self.state,
                    self.widget,
                    if minimum { "setMinimum" } else { "setMaximum" },
                    vm,
                ));
            }
            let raw = number_arg(value, "setMinimum()/setMaximum()", vm)?;
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
                let arg = number_any(stored, is_int, vm);
                emit_if_connected(&self.state, self.widget, "valueChanged", one_arg(arg), vm);
            }
            Ok(())
        }

        fn add_items(&self, texts: Vec<String>, vm: &VirtualMachine) -> PyResult<()> {
            if !self.supported(|widget| widget.supports_combo) {
                return Err(widget_error(&self.state, self.widget, "addItem", vm));
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
                emit_if_connected(
                    &self.state,
                    self.widget,
                    "currentIndexChanged",
                    one_arg(vm.new_pyobj(0i64)),
                    vm,
                );
            }
            Ok(())
        }

        fn apply_current_index(&self, index: i64, vm: &VirtualMachine) -> PyResult<()> {
            let count = {
                let state = self.state.borrow();
                let widget = &state.widgets[self.widget];
                if !widget.supports_combo {
                    return Err(widget_error(
                        &self.state,
                        self.widget,
                        "setCurrentIndex",
                        vm,
                    ));
                }
                widget.items.len()
            };
            if index < -1 || (index >= 0 && index as usize >= count) {
                return Err(index_error("setCurrentIndex", index, count, vm));
            }
            {
                let mut state = self.state.borrow_mut();
                let widget = &mut state.widgets[self.widget];
                widget.current_index = index;
                if index >= 0 {
                    widget.current_text.clear();
                }
            }
            emit_if_connected(
                &self.state,
                self.widget,
                "currentIndexChanged",
                one_arg(vm.new_pyobj(index)),
                vm,
            );
            Ok(())
        }

        fn store_file_name(&self, name: String, vm: &VirtualMachine) -> PyResult<()> {
            if !self.supported(|widget| widget.supports_file_name) {
                return Err(widget_error(&self.state, self.widget, "setFileName", vm));
            }
            {
                let mut state = self.state.borrow_mut();
                state.widgets[self.widget].file_name = name.clone();
            }
            emit_if_connected(
                &self.state,
                self.widget,
                "fileNameChanged",
                one_arg(vm.new_pyobj(name)),
                vm,
            );
            Ok(())
        }

        fn find_child(
            &self,
            query: &PyObjectRef,
            name: Option<&str>,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            let query = query_kind(query, vm)?;
            let found = {
                let state = self.state.borrow();
                find_descendant(&state, self.widget, &query, name)
            };
            Ok(match found {
                Some(found) => found_py(&self.state, found, vm),
                None => vm.ctx.none(),
            })
        }
    }

    #[pyclass(with(GetAttr, Representable))]
    impl PyUiWidget {
        // -- text -------------------------------------------------------------

        #[pymethod]
        fn text(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<String> {
            let state = zelf.state.borrow();
            let widget = &state.widgets[zelf.widget];
            if !widget.supports_text {
                return Err(widget_error(&zelf.state, zelf.widget, "text", vm));
            }
            Ok(widget.text.clone().unwrap_or_default())
        }

        #[pymethod]
        fn setText(zelf: &Py<Self>, text: String, vm: &VirtualMachine) -> PyResult<()> {
            if !zelf.supported(|widget| widget.supports_text) {
                return Err(widget_error(&zelf.state, zelf.widget, "setText", vm));
            }
            {
                let mut state = zelf.state.borrow_mut();
                state.widgets[zelf.widget].text = Some(text.clone());
            }
            emit_if_connected(
                &zelf.state,
                zelf.widget,
                "textChanged",
                one_arg(vm.new_pyobj(text)),
                vm,
            );
            Ok(())
        }

        // -- title (QGroupBox) -------------------------------------------------

        #[pymethod]
        fn title(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<String> {
            let state = zelf.state.borrow();
            let widget = &state.widgets[zelf.widget];
            match &widget.title {
                Some(title) => Ok(title.clone()),
                None => Err(widget_error(&zelf.state, zelf.widget, "title", vm)),
            }
        }

        #[pymethod]
        fn setTitle(zelf: &Py<Self>, title: String, vm: &VirtualMachine) -> PyResult<()> {
            if !zelf.supported(|widget| widget.title.is_some()) {
                return Err(widget_error(&zelf.state, zelf.widget, "setTitle", vm));
            }
            let mut state = zelf.state.borrow_mut();
            state.widgets[zelf.widget].title = Some(title);
            Ok(())
        }

        // -- numeric value -----------------------------------------------------

        #[pymethod]
        fn value(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let (value, is_int) = {
                let state = zelf.state.borrow();
                let widget = &state.widgets[zelf.widget];
                if !widget.supports_value {
                    return Err(widget_error(&zelf.state, zelf.widget, "value", vm));
                }
                (widget.value, widget.value_is_int)
            };
            Ok(number_any(value, is_int, vm))
        }

        #[pymethod]
        fn setValue(zelf: &Py<Self>, value: PyObjectRef, vm: &VirtualMachine) -> PyResult<()> {
            if !zelf.supported(|widget| widget.supports_value) {
                return Err(widget_error(&zelf.state, zelf.widget, "setValue", vm));
            }
            let raw = number_arg(&value, "setValue()", vm)?;
            let (stored, is_int) = {
                let mut state = zelf.state.borrow_mut();
                let widget = &mut state.widgets[zelf.widget];
                let mut stored = if widget.value_is_int {
                    raw.trunc()
                } else {
                    raw
                };
                stored = clamp(stored, widget.minimum, widget.maximum);
                widget.value = stored;
                (stored, widget.value_is_int)
            };
            emit_if_connected(
                &zelf.state,
                zelf.widget,
                "valueChanged",
                one_arg(number_any(stored, is_int, vm)),
                vm,
            );
            Ok(())
        }

        // -- range -------------------------------------------------------------

        #[pymethod]
        fn minimum(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            zelf.range_bound(true, vm)
        }

        #[pymethod]
        fn maximum(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            zelf.range_bound(false, vm)
        }

        #[pymethod]
        fn setMinimum(zelf: &Py<Self>, value: PyObjectRef, vm: &VirtualMachine) -> PyResult<()> {
            zelf.set_range_bound(&value, true, vm)
        }

        #[pymethod]
        fn setMaximum(zelf: &Py<Self>, value: PyObjectRef, vm: &VirtualMachine) -> PyResult<()> {
            zelf.set_range_bound(&value, false, vm)
        }

        // -- checked -----------------------------------------------------------

        #[pymethod]
        fn isChecked(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<bool> {
            let state = zelf.state.borrow();
            let widget = &state.widgets[zelf.widget];
            if !widget.supports_checked {
                return Err(widget_error(&zelf.state, zelf.widget, "isChecked", vm));
            }
            Ok(widget.checked)
        }

        #[pymethod]
        fn setChecked(zelf: &Py<Self>, checked: bool, vm: &VirtualMachine) -> PyResult<()> {
            if !zelf.supported(|widget| widget.supports_checked) {
                return Err(widget_error(&zelf.state, zelf.widget, "setChecked", vm));
            }
            {
                let mut state = zelf.state.borrow_mut();
                state.widgets[zelf.widget].checked = checked;
            }
            let args = one_arg(vm.new_pyobj(checked));
            emit_if_connected(&zelf.state, zelf.widget, "stateChanged", args.clone(), vm);
            emit_if_connected(&zelf.state, zelf.widget, "toggled", args, vm);
            // Qt's `Qt::CheckState`: Unchecked = 0, Checked = 2.
            let check_state = if checked { 2i64 } else { 0i64 };
            emit_if_connected(
                &zelf.state,
                zelf.widget,
                "checkStateChanged",
                one_arg(vm.new_pyobj(check_state)),
                vm,
            );
            Ok(())
        }

        // -- combo box ---------------------------------------------------------

        #[pymethod]
        fn currentIndex(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<i64> {
            let state = zelf.state.borrow();
            let widget = &state.widgets[zelf.widget];
            if !widget.supports_combo {
                return Err(widget_error(&zelf.state, zelf.widget, "currentIndex", vm));
            }
            Ok(widget.current_index)
        }

        #[pymethod]
        fn setCurrentIndex(zelf: &Py<Self>, index: i64, vm: &VirtualMachine) -> PyResult<()> {
            zelf.apply_current_index(index, vm)
        }

        #[pymethod]
        fn currentText(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<String> {
            let state = zelf.state.borrow();
            let widget = &state.widgets[zelf.widget];
            if !widget.supports_combo {
                return Err(widget_error(&zelf.state, zelf.widget, "currentText", vm));
            }
            if widget.current_index >= 0 {
                if let Some(text) = widget.items.get(widget.current_index as usize) {
                    return Ok(text.clone());
                }
            }
            Ok(widget.current_text.clone())
        }

        #[pymethod]
        fn count(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<usize> {
            let state = zelf.state.borrow();
            let widget = &state.widgets[zelf.widget];
            if !widget.supports_combo {
                return Err(widget_error(&zelf.state, zelf.widget, "count", vm));
            }
            Ok(widget.items.len())
        }

        #[pymethod]
        fn itemText(zelf: &Py<Self>, index: i64, vm: &VirtualMachine) -> PyResult<String> {
            let state = zelf.state.borrow();
            let widget = &state.widgets[zelf.widget];
            if !widget.supports_combo {
                return Err(widget_error(&zelf.state, zelf.widget, "itemText", vm));
            }
            if index < 0 || index as usize >= widget.items.len() {
                return Err(index_error("itemText", index, widget.items.len(), vm));
            }
            Ok(widget.items[index as usize].clone())
        }

        #[pymethod]
        fn addItem(zelf: &Py<Self>, text: String, vm: &VirtualMachine) -> PyResult<()> {
            zelf.add_items(vec![text], vm)
        }

        #[pymethod]
        fn addItems(zelf: &Py<Self>, texts: Vec<String>, vm: &VirtualMachine) -> PyResult<()> {
            zelf.add_items(texts, vm)
        }

        #[pymethod]
        fn setCurrentText(zelf: &Py<Self>, text: String, vm: &VirtualMachine) -> PyResult<()> {
            let (supported, found, editable) = {
                let state = zelf.state.borrow();
                let widget = &state.widgets[zelf.widget];
                (
                    widget.supports_combo,
                    widget.items.iter().position(|item| item == &text),
                    widget.editable,
                )
            };
            if !supported {
                return Err(widget_error(&zelf.state, zelf.widget, "setCurrentText", vm));
            }
            if let Some(index) = found {
                return zelf.apply_current_index(index as i64, vm);
            }
            if !editable {
                return Ok(());
            }
            let changed = {
                let mut state = zelf.state.borrow_mut();
                let widget = &mut state.widgets[zelf.widget];
                let changed = widget.current_index != -1 || widget.current_text != text;
                widget.current_index = -1;
                widget.current_text = text;
                changed
            };
            if changed {
                emit_if_connected(
                    &zelf.state,
                    zelf.widget,
                    "currentIndexChanged",
                    one_arg(vm.new_pyobj(-1i64)),
                    vm,
                );
            }
            Ok(())
        }

        /// Remove every item and reset the current index to `-1`.
        #[pymethod]
        fn clear(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<()> {
            let changed = {
                let mut state = zelf.state.borrow_mut();
                let widget = &mut state.widgets[zelf.widget];
                if !widget.supports_combo {
                    return Err(widget_error(&zelf.state, zelf.widget, "clear", vm));
                }
                let changed = widget.current_index != -1;
                widget.items.clear();
                widget.current_index = -1;
                widget.current_text.clear();
                changed
            };
            if changed {
                emit_if_connected(
                    &zelf.state,
                    zelf.widget,
                    "currentIndexChanged",
                    one_arg(vm.new_pyobj(-1i64)),
                    vm,
                );
            }
            Ok(())
        }

        // -- dynamic properties ------------------------------------------------

        /// Read `rawValue`, `color`, `fileName`, `minimum` or `maximum`.
        #[pymethod]
        fn property(zelf: &Py<Self>, name: String, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let state = zelf.state.borrow();
            let widget = &state.widgets[zelf.widget];
            match name.as_str() {
                "rawValue" if widget.supports_raw_value => {
                    // Quantity spin boxes share the numeric value.
                    let value = if widget.supports_value {
                        widget.value
                    } else {
                        widget.raw_value
                    };
                    Ok(vm.new_pyobj(value))
                }
                "color" if widget.supports_color => {
                    let (red, green, blue, alpha) = widget.color.unwrap_or((0, 0, 0, 255));
                    let channels: Vec<PyObjectRef> = [red, green, blue, alpha]
                        .into_iter()
                        .map(|value| vm.new_pyobj(value))
                        .collect();
                    Ok(vm.ctx.new_tuple(channels).into())
                }
                "fileName" if widget.supports_file_name => {
                    Ok(vm.new_pyobj(widget.file_name.clone()))
                }
                "minimum" if widget.supports_range => {
                    let value = widget.minimum.unwrap_or(0.0);
                    Ok(number_any(value, widget.value_is_int, vm))
                }
                "maximum" if widget.supports_range => {
                    let value = widget.maximum.unwrap_or(0.0);
                    Ok(number_any(value, widget.value_is_int, vm))
                }
                _ => Err(property_error(&zelf.state, zelf.widget, &name, vm)),
            }
        }

        /// Write one of the supported dynamic properties.
        #[pymethod]
        fn setProperty(
            zelf: &Py<Self>,
            name: String,
            value: PyObjectRef,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            match name.as_str() {
                "rawValue" => {
                    if !zelf.supported(|widget| widget.supports_raw_value) {
                        return Err(property_error(&zelf.state, zelf.widget, &name, vm));
                    }
                    let raw = number_arg(&value, "setProperty('rawValue')", vm)?;
                    if zelf.supported(|widget| widget.supports_value) {
                        // Keep value()/minimum()/maximum() consistent.
                        return Self::setValue(zelf, value, vm);
                    }
                    {
                        let mut state = zelf.state.borrow_mut();
                        state.widgets[zelf.widget].raw_value = raw;
                    }
                    emit_if_connected(
                        &zelf.state,
                        zelf.widget,
                        "valueChanged",
                        one_arg(vm.new_pyobj(raw)),
                        vm,
                    );
                    Ok(())
                }
                "color" => {
                    if !zelf.supported(|widget| widget.supports_color) {
                        return Err(property_error(&zelf.state, zelf.widget, &name, vm));
                    }
                    let color = parse_color(&value, vm)?;
                    let mut state = zelf.state.borrow_mut();
                    state.widgets[zelf.widget].color = Some(color);
                    Ok(())
                }
                "fileName" => {
                    if !zelf.supported(|widget| widget.supports_file_name) {
                        return Err(property_error(&zelf.state, zelf.widget, &name, vm));
                    }
                    let name = value.clone().try_into_value::<String>(vm).map_err(|_| {
                        vm.new_type_error("setProperty('fileName') expects a string")
                    })?;
                    zelf.store_file_name(name, vm)
                }
                "minimum" => zelf.set_range_bound(&value, true, vm),
                "maximum" => zelf.set_range_bound(&value, false, vm),
                _ => Err(property_error(&zelf.state, zelf.widget, &name, vm)),
            }
        }

        #[pymethod]
        fn fileName(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<String> {
            let state = zelf.state.borrow();
            let widget = &state.widgets[zelf.widget];
            if !widget.supports_file_name {
                return Err(widget_error(&zelf.state, zelf.widget, "fileName", vm));
            }
            Ok(widget.file_name.clone())
        }

        #[pymethod]
        fn setFileName(zelf: &Py<Self>, name: String, vm: &VirtualMachine) -> PyResult<()> {
            zelf.store_file_name(name, vm)
        }

        // -- enabled / visible -------------------------------------------------

        #[pymethod]
        fn isEnabled(zelf: &Py<Self>) -> bool {
            zelf.state.borrow().widgets[zelf.widget].enabled
        }

        #[pymethod]
        fn setEnabled(zelf: &Py<Self>, enabled: bool) {
            zelf.state.borrow_mut().widgets[zelf.widget].enabled = enabled;
        }

        #[pymethod]
        fn isVisible(zelf: &Py<Self>) -> bool {
            zelf.state.borrow().widgets[zelf.widget].visible
        }

        #[pymethod]
        fn setVisible(zelf: &Py<Self>, visible: bool) {
            zelf.state.borrow_mut().widgets[zelf.widget].visible = visible;
        }

        /// No-op visibility helper (there is no window).
        #[pymethod]
        fn show(zelf: &Py<Self>) {
            zelf.state.borrow_mut().widgets[zelf.widget].visible = true;
        }

        /// No-op visibility helper (there is no window).
        #[pymethod]
        fn hide(zelf: &Py<Self>) {
            zelf.state.borrow_mut().widgets[zelf.widget].visible = false;
        }

        // -- layout and parent ------------------------------------------------

        /// The layout this widget owns (`None` when it has none), as in Qt.
        #[pymethod]
        fn layout(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            zelf.owned_layout(vm)
        }

        /// Reparent this widget within the form. `setParent(None)` makes it a
        /// top-level widget in the model. Layout membership is deliberately left
        /// alone: Qt's `setParent` does not remove the widget from a layout.
        #[pymethod]
        fn setParent(
            zelf: &Py<Self>,
            parent: OptionalArg<PyObjectRef>,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            let target = match parent.into_option() {
                None => None,
                Some(value) if vm.is_none(&value) => None,
                Some(value) => {
                    let reference = value.downcast_ref::<PyUiWidget>().ok_or_else(|| {
                        vm.new_type_error("setParent() expects a WidgetProxy or None")
                    })?;
                    if !Rc::ptr_eq(&reference.state, &zelf.state) {
                        return Err(
                            vm.new_value_error("setParent() cannot move a widget between forms")
                        );
                    }
                    Some(reference.widget)
                }
            };
            let mut state = zelf.state.borrow_mut();
            if let Some(target) = target {
                if target == zelf.widget || descendants(&state, zelf.widget).contains(&target) {
                    return Err(
                        vm.new_value_error("setParent() cannot create a cycle in the widget tree")
                    );
                }
            }
            if let Some(old_parent) = state.widgets[zelf.widget].parent.take() {
                state.widgets[old_parent]
                    .children
                    .retain(|child| *child != zelf.widget);
            }
            if let Some(target) = target {
                if !state.widgets[target].children.contains(&zelf.widget) {
                    state.widgets[target].children.push(zelf.widget);
                }
                state.widgets[zelf.widget].parent = Some(target);
            }
            Ok(())
        }

        // -- identity and tree -------------------------------------------------

        #[pymethod]
        fn objectName(zelf: &Py<Self>) -> String {
            zelf.state.borrow().widgets[zelf.widget].name.clone()
        }

        #[pymethod]
        fn className(zelf: &Py<Self>) -> String {
            zelf.state.borrow().widgets[zelf.widget].class.clone()
        }

        /// Direct child widgets, in document order.
        #[pymethod]
        fn children(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            let children = {
                let state = zelf.state.borrow();
                state.widgets[zelf.widget].children.clone()
            };
            let items: Vec<PyObjectRef> = children
                .into_iter()
                .map(|child| widget_py(&zelf.state, child, vm))
                .collect();
            vm.ctx.new_list(items).into()
        }

        /// Find a descendant widget or layout by name, class-name string or class
        /// object. The optional second argument follows Qt: `findChild(cls, "name")`
        /// resolves by name and treats the class as advisory.
        #[pymethod]
        fn findChild(
            zelf: &Py<Self>,
            query: PyObjectRef,
            name: OptionalArg<PyStrRef>,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            let name = name
                .into_option()
                .and_then(|name| name.to_str().map(str::to_string));
            zelf.find_child(&query, name.as_deref(), vm)
        }
    }

    /// Read a `findChild` query: a name/class-name string, or a class object.
    fn query_kind(query: &PyObjectRef, vm: &VirtualMachine) -> PyResult<Query> {
        if let Some(text) = query.downcast_ref::<PyStr>() {
            return Ok(text_query(text.to_str().unwrap_or_default()));
        }
        if let Ok(name) = query.get_attr(vm.ctx.intern_str("__name__"), vm) {
            if let Ok(name) = name.try_into_value::<String>(vm) {
                return Ok(class_query(&name));
            }
        }
        Err(vm.new_type_error("findChild() expects a name, a class-name string or a Qt class"))
    }

    // ---------------------------------------------------------------------------
    // FormProxy
    // ---------------------------------------------------------------------------

    /// A loaded form (the value `PySideUic.loadUi` returns).
    #[pyattr]
    #[pyclass(name = "FormProxy", module = "ferrocad_ui_py")]
    #[derive(PyPayload)]
    pub struct PyUiForm {
        state: State,
    }

    impl fmt::Debug for PyUiForm {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "PyUiForm")
        }
    }

    impl AsSequence for PyUiForm {
        fn as_sequence() -> &'static PySequenceMethods {
            static METHODS: PySequenceMethods = PySequenceMethods {
                length: Some(|seq, _vm| {
                    Ok(PyUiForm::sequence_downcast(seq)
                        .state
                        .borrow()
                        .widgets
                        .len())
                }),
                ..PySequenceMethods::NOT_IMPLEMENTED
            };
            &METHODS
        }
    }

    impl Representable for PyUiForm {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            let state = zelf.state.borrow();
            Ok(format!(
                "FormProxy({} from {}, {} widgets)",
                state.class,
                state.file,
                state.widgets.len()
            ))
        }
    }

    impl GetAttr for PyUiForm {
        fn getattro(zelf: &Py<Self>, name: &Py<PyStr>, vm: &VirtualMachine) -> PyResult {
            let object: PyObjectRef = zelf.to_owned().into();
            if let Ok(value) = object.generic_getattr(name, vm) {
                return Ok(value);
            }
            zelf.lookup(name.to_str().unwrap_or_default(), vm)
        }
    }

    impl PyUiForm {
        /// A widget by Qt object name, or a `QDialog` form's `accepted`/`rejected`.
        fn lookup(&self, name: &str, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            if name.starts_with("__") {
                return Err(attribute_missing(&self.state, name, vm));
            }
            let (root, dialog) = {
                let state = self.state.borrow();
                (state.root, is_dialog_root(&state.widgets[state.root].class))
            };
            if dialog && matches!(name, "accepted" | "rejected") {
                let callbacks = self.state.borrow_mut().callbacks(root, name);
                return Ok(signal_py(&self.state, root, name, callbacks, vm));
            }
            let widget = {
                let state = self.state.borrow();
                match state.by_name.get(name) {
                    Some(widget) => Some(*widget),
                    None => None,
                }
            };
            match widget {
                Some(widget) => Ok(widget_py(&self.state, widget, vm)),
                None => {
                    let state = self.state.borrow();
                    let names: Vec<&str> = state
                        .widgets
                        .iter()
                        .map(|widget| widget.name.as_str())
                        .collect();
                    Err(vm.new_attribute_error(format!(
                        "form '{}' ({}) has no widget '{}'; available widgets: {}",
                        state.class,
                        state.file,
                        name,
                        names.join(", ")
                    )))
                }
            }
        }

        fn check_dialog(&self, operation: &str, vm: &VirtualMachine) -> PyResult<usize> {
            let state = self.state.borrow();
            let root_class = &state.widgets[state.root].class;
            if !is_dialog_root(root_class) {
                return Err(vm.new_attribute_error(format!(
                    "form '{}' ({}) is a {}; {} is only available on QDialog roots",
                    state.class, state.file, root_class, operation
                )));
            }
            Ok(state.root)
        }

        fn finish_dialog(&self, accepted: bool, vm: &VirtualMachine) -> PyResult<()> {
            let root = self.check_dialog(if accepted { "accept()" } else { "reject()" }, vm)?;
            self.state.borrow_mut().result = if accepted { 1 } else { 0 };
            let signal = if accepted { "accepted" } else { "rejected" };
            emit_if_connected(&self.state, root, signal, Vec::new(), vm);
            Ok(())
        }
    }

    #[pyclass(with(GetAttr, Representable, AsSequence))]
    impl PyUiForm {
        /// Find a widget or layout by name, class-name string or class object.
        #[pymethod]
        fn findChild(
            zelf: &Py<Self>,
            query: PyObjectRef,
            name: OptionalArg<PyStrRef>,
            vm: &VirtualMachine,
        ) -> PyResult<PyObjectRef> {
            let name = name
                .into_option()
                .and_then(|name| name.to_str().map(str::to_string));
            let query = query_kind(&query, vm)?;
            let found = {
                let state = zelf.state.borrow();
                find_in_form(&state, &query, name.as_deref())
            };
            Ok(match found {
                Some(found) => found_py(&zelf.state, found, vm),
                None => vm.ctx.none(),
            })
        }

        /// The form's window title (`windowTitle` in the `.ui`).
        #[pymethod]
        fn windowTitle(zelf: &Py<Self>) -> String {
            zelf.state.borrow().window_title.clone()
        }

        #[pymethod]
        fn setWindowTitle(zelf: &Py<Self>, title: String) {
            zelf.state.borrow_mut().window_title = title;
        }

        /// No-op: there is no window.
        #[pymethod]
        fn show(_zelf: &Py<Self>) {}

        /// No-op: there is no window.
        #[pymethod]
        fn hide(_zelf: &Py<Self>) {}

        /// No-op: there is no window or event loop.
        #[pymethod]
        fn close(_zelf: &Py<Self>) {}

        /// Accept a dialog: set the result to `QDialog::Accepted` (1) and emit the
        /// form-level `accepted` signal.
        #[pymethod]
        fn accept(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<()> {
            zelf.finish_dialog(true, vm)
        }

        /// Reject a dialog: set the result to `QDialog::Rejected` (0) and emit the
        /// form-level `rejected` signal.
        #[pymethod]
        fn reject(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<()> {
            zelf.finish_dialog(false, vm)
        }

        /// The stored dialog result without running an event loop.
        #[pymethod]
        fn exec_(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<i64> {
            zelf.check_dialog("exec_()", vm)?;
            Ok(zelf.state.borrow().result)
        }

        /// The stored dialog result: 1 after `accept()`, 0 otherwise.
        #[pymethod]
        fn result(zelf: &Py<Self>, vm: &VirtualMachine) -> PyResult<i64> {
            zelf.check_dialog("result()", vm)?;
            Ok(zelf.state.borrow().result)
        }

        /// The layout installed on the root widget (`None` when it has none).
        #[pymethod]
        fn layout(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            let layout = {
                let state = zelf.state.borrow();
                state.widgets[state.root].layout
            };
            match layout {
                Some(layout) => layout_py(&zelf.state, layout, vm),
                None => vm.ctx.none(),
            }
        }

        /// The root widget's Qt object name.
        #[pymethod]
        fn objectName(zelf: &Py<Self>) -> String {
            let state = zelf.state.borrow();
            state.widgets[state.root].name.clone()
        }

        /// The form's `<class>` name.
        #[pymethod]
        fn className(zelf: &Py<Self>) -> String {
            zelf.state.borrow().class.clone()
        }

        /// The root widget's direct children.
        #[pymethod]
        fn children(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            let children = {
                let state = zelf.state.borrow();
                state.widgets[state.root].children.clone()
            };
            let items: Vec<PyObjectRef> = children
                .into_iter()
                .map(|child| widget_py(&zelf.state, child, vm))
                .collect();
            vm.ctx.new_list(items).into()
        }
    }

    // ---------------------------------------------------------------------------
    // LayoutProxy / LayoutItemProxy
    // ---------------------------------------------------------------------------

    /// A layout in a loaded form.
    #[pyattr]
    #[pyclass(name = "LayoutProxy", module = "ferrocad_ui_py")]
    #[derive(PyPayload)]
    pub struct PyUiLayout {
        state: State,
        layout: usize,
    }

    impl fmt::Debug for PyUiLayout {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "PyUiLayout({})", self.layout)
        }
    }

    impl Representable for PyUiLayout {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            let state = zelf.state.borrow();
            let layout = &state.layouts[zelf.layout];
            Ok(format!(
                "LayoutProxy({} '{}', {} items)",
                layout.class,
                layout.name.as_deref().unwrap_or("<unnamed>"),
                layout.items.len()
            ))
        }
    }

    impl PyUiLayout {
        fn error(&self, member: &str, vm: &VirtualMachine) -> PyBaseExceptionRef {
            vm.new_attribute_error(self.state.borrow().layout_message(self.layout, member))
        }

        fn entry(&self, index: i64, vm: &VirtualMachine) -> PyResult<LayoutEntry> {
            let state = self.state.borrow();
            let layout = &state.layouts[self.layout];
            if index < 0 || index as usize >= layout.items.len() {
                return Err(index_error("layout item", index, layout.items.len(), vm));
            }
            Ok(layout.items[index as usize].clone())
        }

        fn item_py(&self, entry: LayoutEntry, vm: &VirtualMachine) -> PyObjectRef {
            PyUiLayoutItem {
                state: self.state.clone(),
                entry,
            }
            .into_ref(&vm.ctx)
            .into()
        }

        fn check_grid_args(
            &self,
            what: &str,
            row: Option<u32>,
            column: Option<u32>,
            rowspan: u32,
            colspan: u32,
            class: &str,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            if !is_grid_layout(class)
                && (row.is_some() || column.is_some() || rowspan != 1 || colspan != 1)
            {
                return Err(vm.new_value_error(format!(
                    "{class} only supports {what} without row/column/spans"
                )));
            }
            Ok(())
        }
    }

    #[pyclass(with(Representable))]
    impl PyUiLayout {
        #[pymethod]
        fn count(zelf: &Py<Self>) -> usize {
            zelf.state.borrow().layouts[zelf.layout].items.len()
        }

        /// The item at `index`, or `None` when the index is out of range.
        #[pymethod]
        fn itemAt(zelf: &Py<Self>, index: i64, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let count = zelf.state.borrow().layouts[zelf.layout].items.len();
            if index < 0 || index as usize >= count {
                return Ok(vm.ctx.none());
            }
            let entry = zelf.entry(index, vm)?;
            Ok(zelf.item_py(entry, vm))
        }

        /// Remove and return the item at `index`. The widget/layout itself is not
        /// deleted and keeps its parent, as Qt's `takeAt` behaves.
        #[pymethod]
        fn takeAt(zelf: &Py<Self>, index: i64, vm: &VirtualMachine) -> PyResult<PyObjectRef> {
            let entry = {
                let mut state = zelf.state.borrow_mut();
                let count = state.layouts[zelf.layout].items.len();
                if index < 0 || index as usize >= count {
                    return Err(index_error("takeAt", index, count, vm));
                }
                let entry = state.layouts[zelf.layout].items.remove(index as usize);
                if let LayoutEntryKind::Widget(widget) = entry.kind {
                    if state.widgets[widget].owning_layout == Some(zelf.layout) {
                        state.widgets[widget].owning_layout = None;
                    }
                }
                entry
            };
            Ok(zelf.item_py(entry, vm))
        }

        /// Append a widget. A grid layout accepts `row`/`column`/`rowspan`/
        /// `colspan`; other layouts only accept `addWidget(widget)`.
        #[pymethod]
        fn addWidget(
            zelf: &Py<Self>,
            widget: PyRef<PyUiWidget>,
            row: OptionalArg<u32>,
            column: OptionalArg<u32>,
            rowspan: OptionalArg<u32>,
            colspan: OptionalArg<u32>,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            let (row, column) = (row.into_option(), column.into_option());
            let rowspan = rowspan.into_option().unwrap_or(1);
            let colspan = colspan.into_option().unwrap_or(1);
            if !Rc::ptr_eq(&widget.state, &zelf.state) {
                return Err(vm.new_value_error("addWidget() cannot move a widget between forms"));
            }
            let index = widget.widget;

            let mut state = zelf.state.borrow_mut();
            let class = state.layouts[zelf.layout].class.clone();
            zelf.check_grid_args(
                "addWidget(widget)",
                row,
                column,
                rowspan,
                colspan,
                &class,
                vm,
            )?;
            let parent = state.layouts[zelf.layout].parent;
            if let Some(parent) = parent {
                if parent == index || descendants(&state, index).contains(&parent) {
                    return Err(
                        vm.new_value_error("addWidget() cannot create a cycle in the widget tree")
                    );
                }
            }

            // A widget has exactly one layout; adding it moves it.
            if let Some(old) = state.widgets[index].owning_layout.take() {
                state.layouts[old].items.retain(
                    |entry| !matches!(entry.kind, LayoutEntryKind::Widget(w) if w == index),
                );
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
            state.layouts[zelf.layout].items.push(LayoutEntry {
                kind: LayoutEntryKind::Widget(index),
                row,
                column,
                row_span: rowspan,
                column_span: colspan,
            });
            state.widgets[index].owning_layout = Some(zelf.layout);
            Ok(())
        }

        /// Append a nested layout, with the same grid arguments as `addWidget`.
        #[pymethod]
        fn addLayout(
            zelf: &Py<Self>,
            layout: PyRef<PyUiLayout>,
            row: OptionalArg<u32>,
            column: OptionalArg<u32>,
            rowspan: OptionalArg<u32>,
            colspan: OptionalArg<u32>,
            vm: &VirtualMachine,
        ) -> PyResult<()> {
            let (row, column) = (row.into_option(), column.into_option());
            let rowspan = rowspan.into_option().unwrap_or(1);
            let colspan = colspan.into_option().unwrap_or(1);
            if !Rc::ptr_eq(&layout.state, &zelf.state) {
                return Err(vm.new_value_error("addLayout() cannot move a layout between forms"));
            }
            let child = layout.layout;
            if child == zelf.layout {
                return Err(vm.new_value_error("addLayout() cannot add a layout to itself"));
            }

            let mut state = zelf.state.borrow_mut();
            let class = state.layouts[zelf.layout].class.clone();
            zelf.check_grid_args(
                "addLayout(layout)",
                row,
                column,
                rowspan,
                colspan,
                &class,
                vm,
            )?;
            if layout_contains(&state, child, zelf.layout) {
                return Err(
                    vm.new_value_error("addLayout() cannot create a cycle in the layout tree")
                );
            }
            for candidate in &mut state.layouts {
                candidate.items.retain(
                    |entry| !matches!(entry.kind, LayoutEntryKind::Layout(id) if id == child),
                );
            }
            state.layouts[child].parent = state.layouts[zelf.layout].parent;
            state.layouts[zelf.layout].items.push(LayoutEntry {
                kind: LayoutEntryKind::Layout(child),
                row,
                column,
                row_span: rowspan,
                column_span: colspan,
            });
            Ok(())
        }

        /// The layout's Qt object name (empty when unnamed).
        #[pymethod]
        fn objectName(zelf: &Py<Self>) -> String {
            zelf.state.borrow().layouts[zelf.layout]
                .name
                .clone()
                .unwrap_or_default()
        }

        /// The layout's Qt class name, for example `QGridLayout`.
        #[pymethod]
        fn className(zelf: &Py<Self>) -> String {
            zelf.state.borrow().layouts[zelf.layout].class.clone()
        }
    }

    impl GetAttr for PyUiLayout {
        fn getattro(zelf: &Py<Self>, name: &Py<PyStr>, vm: &VirtualMachine) -> PyResult {
            let object: PyObjectRef = zelf.to_owned().into();
            if let Ok(value) = object.generic_getattr(name, vm) {
                return Ok(value);
            }
            Err(zelf.error(name.to_str().unwrap_or_default(), vm))
        }
    }

    /// The value `LayoutProxy.takeAt()`/`itemAt()` returns: a detached view of one
    /// layout item.
    #[pyattr]
    #[pyclass(name = "LayoutItemProxy", module = "ferrocad_ui_py")]
    #[derive(PyPayload)]
    pub struct PyUiLayoutItem {
        state: State,
        entry: LayoutEntry,
    }

    impl fmt::Debug for PyUiLayoutItem {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "PyUiLayoutItem")
        }
    }

    impl Representable for PyUiLayoutItem {
        fn repr_str(zelf: &Py<Self>, _vm: &VirtualMachine) -> PyResult<String> {
            Ok(match zelf.entry.kind {
                LayoutEntryKind::Widget(widget) => {
                    let state = zelf.state.borrow();
                    format!("LayoutItemProxy(widget '{}')", state.widgets[widget].name)
                }
                LayoutEntryKind::Layout(layout) => {
                    let state = zelf.state.borrow();
                    format!(
                        "LayoutItemProxy(layout '{}')",
                        state.layouts[layout].name.as_deref().unwrap_or("<unnamed>")
                    )
                }
                LayoutEntryKind::Spacer(spacer) => format!("LayoutItemProxy(spacer #{spacer})"),
            })
        }
    }

    impl GetAttr for PyUiLayoutItem {
        fn getattro(zelf: &Py<Self>, name: &Py<PyStr>, vm: &VirtualMachine) -> PyResult {
            let object: PyObjectRef = zelf.to_owned().into();
            if let Ok(value) = object.generic_getattr(name, vm) {
                return Ok(value);
            }
            let state = zelf.state.borrow();
            Err(vm.new_attribute_error(format!(
                "LayoutItem has no attribute '{}' (form {})",
                name.to_str().unwrap_or_default(),
                state.file
            )))
        }
    }

    #[pyclass(with(GetAttr, Representable))]
    impl PyUiLayoutItem {
        /// The widget held by the item, or `None` for a nested layout/spacer.
        #[pymethod]
        fn widget(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            match zelf.entry.kind {
                LayoutEntryKind::Widget(widget) => widget_py(&zelf.state, widget, vm),
                _ => vm.ctx.none(),
            }
        }

        /// The nested layout held by the item, or `None` for a widget/spacer.
        #[pymethod]
        fn layout(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            match zelf.entry.kind {
                LayoutEntryKind::Layout(layout) => layout_py(&zelf.state, layout, vm),
                _ => vm.ctx.none(),
            }
        }

        #[pymethod]
        fn row(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            match zelf.entry.row {
                Some(row) => vm.new_pyobj(row),
                None => vm.ctx.none(),
            }
        }

        #[pymethod]
        fn column(zelf: &Py<Self>, vm: &VirtualMachine) -> PyObjectRef {
            match zelf.entry.column {
                Some(column) => vm.new_pyobj(column),
                None => vm.ctx.none(),
            }
        }

        #[pymethod]
        fn rowSpan(zelf: &Py<Self>) -> u32 {
            zelf.entry.row_span
        }

        #[pymethod]
        fn colSpan(zelf: &Py<Self>) -> u32 {
            zelf.entry.column_span
        }
    }

    // ---------------------------------------------------------------------------
    // The module
    // ---------------------------------------------------------------------------

    #[pyattr]
    const __version__: &str = "0.1.5";

    /// A Qt Designer `.ui` form could not be read or parsed.
    #[pyattr]
    #[pyexception(name = "UiLoadError", base = PyValueError, impl)]
    #[derive(Debug)]
    #[repr(transparent)]
    pub struct UiLoadError(PyValueError);

    fn load_error(
        message: String,
        vm: &VirtualMachine,
    ) -> rustpython_vm::builtins::PyBaseExceptionRef {
        vm.new_exception_msg(UiLoadError::static_type().to_owned(), message.into())
    }

    /// Load a `.ui` form and return its `FormProxy`.
    ///
    /// `source` is a filesystem path, UTF-8 bytes, or a `QFile`-like object with
    /// `readAll()`/`data()`. `name` overrides the label used in error messages
    /// (the facade passes the `:/ui/...` resource path there).
    #[pyfunction]
    fn load_ui(
        source: PyObjectRef,
        name: OptionalArg<PyStrRef>,
        vm: &VirtualMachine,
    ) -> PyResult<PyObjectRef> {
        let label = name
            .into_option()
            .and_then(|name| name.to_str().map(str::to_string));

        // A filesystem path.
        if let Some(path) = source.downcast_ref::<PyStr>() {
            let path = path.to_str().unwrap_or_default().to_string();
            if path.starts_with(":/") {
                return Err(vm.new_value_error(format!(
                    "resource path {path:?} must be resolved by FreeCADGui.PySideUic.loadUi"
                )));
            }
            let label = label.unwrap_or_else(|| path.clone());
            let form = Form::from_path(&path).map_err(|error| load_error(error.to_string(), vm))?;
            return Ok(form_py(form, label, vm));
        }

        // Raw bytes.
        if let Some(bytes) = source.downcast_ref::<PyBytes>() {
            let text = std::str::from_utf8(bytes.as_bytes())
                .map_err(|error| load_error(format!("<bytes>: not UTF-8: {error}"), vm))?;
            let label = label.unwrap_or_else(|| "<bytes>".to_owned());
            let form = Form::parse_named(text, &label)
                .map_err(|error| load_error(error.to_string(), vm))?;
            return Ok(form_py(form, label, vm));
        }

        // A `QFile`-like object.
        if source.get_attr(vm.ctx.intern_str("readAll"), vm).is_err()
            && source.get_attr(vm.ctx.intern_str("data"), vm).is_err()
        {
            return Err(vm.new_type_error(format!(
                "load_ui() expects a path, bytes or a QFile-like object, got {}",
                source.class().name()
            )));
        }
        let label = label
            .or_else(|| file_name(&source, vm))
            .unwrap_or_else(|| "<file>".to_owned());
        let data = if let Ok(read_all) = source.get_attr(vm.ctx.intern_str("readAll"), vm) {
            read_all.call((), vm)?
        } else {
            source
                .get_attr(vm.ctx.intern_str("data"), vm)?
                .call((), vm)?
        };
        let bytes = bytes_from_object(&data, vm)?;
        let text = String::from_utf8(bytes)
            .map_err(|error| load_error(format!("{label}: not UTF-8: {error}"), vm))?;
        let form =
            Form::parse_named(&text, &label).map_err(|error| load_error(error.to_string(), vm))?;
        Ok(form_py(form, label, vm))
    }

    /// The label of a `QFile`-like object (`fileName()`), when it has one.
    fn file_name(source: &PyObjectRef, vm: &VirtualMachine) -> Option<String> {
        let name = source
            .get_attr(vm.ctx.intern_str("fileName"), vm)
            .ok()?
            .call((), vm)
            .ok()?;
        name.try_into_value::<String>(vm).ok()
    }

    fn bytes_from_object(object: &PyObjectRef, vm: &VirtualMachine) -> PyResult<Vec<u8>> {
        if let Some(bytes) = object.downcast_ref::<PyBytes>() {
            return Ok(bytes.as_bytes().to_vec());
        }
        if let Some(text) = object.downcast_ref::<PyStr>() {
            return Ok(text.to_str().unwrap_or_default().as_bytes().to_vec());
        }
        if let Ok(data) = object.get_attr(vm.ctx.intern_str("data"), vm) {
            let data = data.call((), vm)?;
            // A `data()` that returns the receiver would recurse forever.
            if !data.is(object) {
                return bytes_from_object(&data, vm);
            }
        }
        if let Ok(bytes) = object.clone().try_into_value::<Vec<u8>>(vm) {
            return Ok(bytes);
        }
        Err(vm.new_type_error(format!(
            "expected bytes, str or a QByteArray-like object, got {}",
            object.class().name()
        )))
    }

    fn form_py(form: Form, label: String, vm: &VirtualMachine) -> PyObjectRef {
        let state: State = std::rc::Rc::new(std::cell::RefCell::new(
            ferrocad_ui::state::build_state(&form, label),
        ));
        PyUiForm { state }.into_ref(&vm.ctx).into()
    }
}

pub(crate) use ferrocad_ui_py::module_def as ui_module_def;
