//! Interactive tools: a Python base class, and a Rust adapter that drives it.
//!
//! A script subclasses [`Tool`] and overrides `on_event`; the host wraps the instance in
//! a [`PyTool`], which implements the Rust [`ferrocad_stage::Tool`] interface, so a
//! scripted tool can be dropped into the viewport exactly like `PolylineTool`.

use ferrocad_stage as fs;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;

use crate::input::PointerEvent;
use crate::shape::ShapeItem;

/// An interactive tool, for Python subclassing.
///
/// Override [`on_event`](Self::on_event) and manage the transient visuals with
/// [`add_item`](Self::add_item)/[`set_items`](Self::set_items)/[`clear_items`](Self::clear_items).
#[pyclass(subclass, module = "ferrocad_stage", name = "Tool")]
pub struct Tool {
    pub(crate) items: Vec<fs::ShapeItem>,
}

#[pymethods]
impl Tool {
    #[new]
    fn new() -> Self {
        Self { items: Vec::new() }
    }

    /// React to a pointer event. **Override this in a subclass.** Returns whether the
    /// event was consumed (a host may use it to stop propagation).
    #[pyo3(signature = (event))]
    fn on_event(&self, event: &PointerEvent) -> PyResult<bool> {
        let _ = event;
        Ok(false)
    }

    /// Append a transient visual.
    fn add_item(&mut self, item: &ShapeItem) {
        self.items.push(item.inner.clone());
    }

    /// Replace the transient visuals.
    fn set_items(&mut self, items: Vec<PyRef<'_, ShapeItem>>) {
        self.items = items.iter().map(|item| item.inner.clone()).collect();
    }

    /// Drop the transient visuals.
    fn clear_items(&mut self) {
        self.items.clear();
    }

    /// The transient visuals (as copies).
    #[getter]
    fn items(&self) -> Vec<ShapeItem> {
        self.items.iter().cloned().map(ShapeItem::wrap).collect()
    }
}

/// Register this module's classes.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Tool>()?;
    Ok(())
}

/// A Python [`Tool`] instance driven through the native [`ferrocad_stage::Tool`]
/// interface.
///
/// The transient items are cached Rust-side and refreshed after every event (Python owns
/// the list, but the renderer needs a `&[ShapeItem]`).
pub struct PyTool {
    object: Py<PyAny>,
    items: Vec<fs::ShapeItem>,
}

impl PyTool {
    /// Wrap a Python `Tool` (or subclass) instance.
    pub fn new(py: Python<'_>, object: Py<PyAny>) -> PyResult<Self> {
        let mut tool = Self {
            object,
            items: Vec::new(),
        };
        tool.refresh(py)?;
        Ok(tool)
    }

    /// The wrapped Python object.
    pub fn object(&self) -> &Py<PyAny> {
        &self.object
    }

    /// Re-read the transient items from the Python instance.
    fn refresh(&mut self, py: Python<'_>) -> PyResult<()> {
        let bound = self.object.bind(py).downcast::<Tool>().map_err(|_| {
            PyTypeError::new_err("expected a `ferrocad.stage.Tool` instance")
        })?;
        self.items = bound.borrow().items.clone();
        Ok(())
    }
}

impl fs::Tool for PyTool {
    fn on_event(&mut self, event: &fs::PointerEvent, snap: Option<fs::Snap>) {
        // A raising handler is reported, not propagated: one broken tool must not take
        // down the frame loop.
        Python::with_gil(|py| {
            let event = match Bound::new(py, PointerEvent::wrap(*event, snap)) {
                Ok(event) => event,
                Err(err) => {
                    err.print(py);
                    return;
                }
            };
            if let Err(err) = self.object.bind(py).call_method1("on_event", (event,)) {
                err.print(py);
            }
            if let Err(err) = self.refresh(py) {
                err.print(py);
            }
        });
    }

    fn items(&self) -> &[fs::ShapeItem] {
        &self.items
    }
}

/// Wrap a Python `Tool` instance in the native [`fs::Tool`] interface.
pub fn extract_tool(py: Python<'_>, object: Py<PyAny>) -> PyResult<PyTool> {
    PyTool::new(py, object)
}
