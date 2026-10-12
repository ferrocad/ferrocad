//! `Stage`: an ordered collection of declarative visual items.
//!
//! The Python stage holds the same flat list of [`ShapeItem`](crate::ShapeItem)s as
//! [`ferrocad_stage::Stage`]; [`batch_count`](Stage::batch_count) compiles it, so a
//! script can assert what it will draw without a renderer.

use ferrocad_stage as fs;
use pyo3::exceptions::PyTypeError;
use pyo3::prelude::*;

use crate::shape::ShapeItem;

/// A stage of items (rebuilt cheaply, so a host can reassemble it each frame).
#[pyclass(module = "ferrocad_stage", name = "Stage")]
#[derive(Default)]
pub struct Stage {
    pub(crate) inner: fs::Stage,
}

#[pymethods]
impl Stage {
    #[new]
    fn new() -> Self {
        Self {
            inner: fs::Stage::new(),
        }
    }

    /// Append a copy of `item` (drawn last).
    fn add(&mut self, item: &ShapeItem) {
        self.inner.add_item(item.inner.clone());
    }

    /// Drop every item.
    fn clear(&mut self) {
        self.inner = fs::Stage::new();
    }

    /// The number of items.
    fn __len__(&self) -> usize {
        self.inner.len()
    }

    /// Whether the stage has no items.
    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// How many draw packets the stage compiles to right now.
    fn batch_count(&self) -> usize {
        self.inner.to_queue().len()
    }

    /// The pick ids of the items, in draw order.
    fn entities(&self) -> Vec<u32> {
        self.inner.items().iter().map(|item| item.entity_id()).collect()
    }

    fn __repr__(&self) -> String {
        format!("Stage(items={})", self.inner.len())
    }
}

/// Register this module's classes.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Stage>()?;
    Ok(())
}

/// Extract the native stage from a Python [`Stage`], for a host to render. The analogue
/// of `ferrocad_inventor_py::extract_scene`.
pub fn extract_stage(object: &Bound<'_, PyAny>) -> PyResult<fs::Stage> {
    let stage = object
        .downcast::<Stage>()
        .map_err(|_| PyTypeError::new_err("expected a `ferrocad.stage.Stage`"))?;
    Ok(stage.borrow().inner.clone())
}
