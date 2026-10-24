//! # `ferrocad_ui_py` — headless proxies for Qt Designer forms
//!
//! Q2a of the Qt-reduction plan: expose the typed form tree from
//! [`ferrocad_ui`] to Python as `FormProxy` / `WidgetProxy` / `SignalProxy`.
//! `FreeCADGui.PySideUic.loadUi(path_or_file)` is the Python facade; this crate
//! is the extension module behind it.
//!
//! The surface is deliberately small:
//!
//! * [`FormProxy`] looks widgets up by name, resolves `findChild`, and carries
//!   the root operations (`setWindowTitle`, `show`/`hide`/`close`,
//!   `accept`/`reject` on `QDialog` roots).
//! * [`WidgetProxy`] exposes `text`/`setText`, `value`/`setValue`,
//!   `isChecked`/`setChecked`, `currentIndex`/`setCurrentIndex`/`currentText`,
//!   `isEnabled`/`setEnabled`, `setVisible`, `objectName`, `children` and
//!   `findChild` for the widget classes that actually have those members.
//! * [`SignalProxy`] is a callback list with `connect`, `disconnect` and
//!   `emit`; the common signals are attributes on the widget.
//!
//! Anything outside that set raises `AttributeError` naming the form, widget
//! class, widget name and member. There is no silent `None`, no Qt object model
//! and no rendering.
//!
//! # Loading
//!
//! `load_ui(source, name=None)` accepts a filesystem path (`str`), UTF-8
//! `bytes`, or a `QFile`-like object with `readAll()`/`data()` (the repository's
//! pure-Python `PySide` shim satisfies this). `:/ui/...` resource paths are
//! resolved by the `FreeCADGui.PySideUic` facade, which owns the mod resource
//! lookup; calling this module directly with a `:/` path is an error.
//!
//! # Building
//!
//! Plain `cargo build -p ferrocad_ui_py` produces `libferrocad_ui_py.so` (or the
//! platform equivalent); Python imports it under the name `ferrocad_ui_py` (the
//! packaged name is `ferrocad_ui_py.abi3.so`). Building against CPython newer
//! than PyO3's declared maximum needs
//! `PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1` (the repository's `build.sh` sets it).

#![forbid(unsafe_code)]
// The proxy names are Qt's public Python API (`setText`, `findChild`, …).
#![allow(non_snake_case)]

mod proxy;
mod state;

use ferrocad_ui::Form;
use pyo3::exceptions::{PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyString};

pub use proxy::{FormProxy, SignalProxy, WidgetProxy};

pyo3::create_exception!(
    ferrocad_ui_py,
    UiLoadError,
    PyValueError,
    "A Qt Designer .ui form could not be read or parsed."
);

/// Load a `.ui` form and return its [`FormProxy`].
///
/// `source` is a filesystem path, UTF-8 bytes, or a `QFile`-like object with
/// `readAll()`/`data()`. `name` overrides the label used in error messages
/// (the facade passes the `:/ui/...` resource path there).
#[pyfunction]
#[pyo3(signature = (source, name=None))]
fn load_ui(
    py: Python<'_>,
    source: &Bound<'_, PyAny>,
    name: Option<String>,
) -> PyResult<Py<FormProxy>> {
    let (form, label) = read_source(source, name)?;
    let state = state::build_state(&form, label);
    Ok(Py::new(py, FormProxy::new(state))?)
}

fn read_source(
    source: &Bound<'_, PyAny>,
    name: Option<String>,
) -> PyResult<(Form, String)> {
    if let Ok(path) = source.downcast::<PyString>() {
        let path = path.to_str()?.to_owned();
        if path.starts_with(":/") {
            return Err(PyValueError::new_err(format!(
                "resource path {path:?} must be resolved by FreeCADGui.PySideUic.loadUi"
            )));
        }
        let label = name.unwrap_or_else(|| path.clone());
        let form = Form::from_path(&path).map_err(|error| UiLoadError::new_err(error.to_string()))?;
        return Ok((form, label));
    }

    if let Ok(bytes) = source.downcast::<PyBytes>() {
        let text = std::str::from_utf8(bytes.as_bytes()).map_err(|error| {
            UiLoadError::new_err(format!("<bytes>: not UTF-8: {error}"))
        })?;
        let label = name.unwrap_or_else(|| "<bytes>".to_owned());
        let form = Form::parse_named(text, &label)
            .map_err(|error| UiLoadError::new_err(error.to_string()))?;
        return Ok((form, label));
    }

    if !source.hasattr("readAll")? && !source.hasattr("data")? {
        return Err(PyTypeError::new_err(format!(
            "load_ui() expects a path, bytes or a QFile-like object, got {}",
            source.get_type().name()?
        )));
    }

    let label = name
        .or_else(|| file_name(source))
        .unwrap_or_else(|| "<file>".to_owned());
    let data = if source.hasattr("readAll")? {
        source.call_method0("readAll")?
    } else {
        source.call_method0("data")?
    };
    let bytes = bytes_from_object(&data)?;
    let text = String::from_utf8(bytes).map_err(|error| {
        UiLoadError::new_err(format!("{label}: not UTF-8: {error}"))
    })?;
    let form = Form::parse_named(&text, &label)
        .map_err(|error| UiLoadError::new_err(error.to_string()))?;
    Ok((form, label))
}

fn file_name(source: &Bound<'_, PyAny>) -> Option<String> {
    let method = source.getattr("fileName").ok()?;
    let value = method.call0().ok()?;
    value.extract::<String>().ok()
}

fn bytes_from_object(object: &Bound<'_, PyAny>) -> PyResult<Vec<u8>> {
    if let Ok(bytes) = object.downcast::<PyBytes>() {
        return Ok(bytes.as_bytes().to_vec());
    }
    if let Ok(text) = object.downcast::<PyString>() {
        return Ok(text.to_str()?.as_bytes().to_vec());
    }
    if object.hasattr("data")? {
        let data = object.call_method0("data")?;
        // A `data()` that returns the receiver would recurse forever; fall
        // through to the sequence conversion instead.
        if !data.is(object) {
            return bytes_from_object(&data);
        }
    }
    if let Ok(bytes) = object.extract::<Vec<u8>>() {
        return Ok(bytes);
    }
    Err(PyTypeError::new_err(format!(
        "expected bytes, str or a QByteArray-like object, got {}",
        object.get_type().name()?
    )))
}

/// The `ferrocad_ui_py` extension module.
#[pymodule]
fn ferrocad_ui_py(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("__version__", env!("CARGO_PKG_VERSION"))?;
    module.add("UiLoadError", module.py().get_type::<UiLoadError>())?;
    module.add_class::<FormProxy>()?;
    module.add_class::<WidgetProxy>()?;
    module.add_class::<SignalProxy>()?;
    module.add_function(wrap_pyfunction!(load_ui, module)?)?;
    Ok(())
}
