//! The interactive transform gizmo, exposed to Python.
//!
//! A faithful wrap of [`ferrocad_stage::TransformGizmo`]: the constraint-projection math
//! needs a [`Camera`](crate::Camera), and the drag/commit deltas come back as
//! [`GizmoDragEvent`]/[`GizmoCommitEvent`] objects carrying 16-float (row-major) matrices.

use ferrocad_stage as fs;
use ferrocad_types::Vector3;
use pyo3::prelude::*;

use crate::camera::Camera;
use crate::enums::{GizmoHandle, GizmoMode};
use crate::input::PointerEvent;

/// A gizmo drag tick.
#[pyclass(module = "ferrocad_stage", name = "GizmoDragEvent")]
#[derive(Clone)]
pub struct GizmoDragEvent {
    pub(crate) inner: fs::GizmoDragEvent,
}

#[pymethods]
impl GizmoDragEvent {
    /// The targeted entity id.
    #[getter]
    fn target_id(&self) -> u32 {
        self.inner.target_id
    }

    /// The handle being dragged.
    #[getter]
    fn active_handle(&self) -> GizmoHandle {
        self.inner.active_handle.into()
    }

    /// The transform delta since the previous tick (16 row-major floats).
    #[getter]
    fn delta_transform(&self) -> Vec<f64> {
        self.inner.delta_transform.values().to_vec()
    }

    /// The transform accumulated since the drag began (16 row-major floats).
    #[getter]
    fn total_transform(&self) -> Vec<f64> {
        self.inner.total_transform.values().to_vec()
    }

    fn __repr__(&self) -> String {
        format!(
            "GizmoDragEvent(target_id={}, handle={:?})",
            self.inner.target_id, self.inner.active_handle
        )
    }
}

/// A gizmo drag commit (on release).
#[pyclass(module = "ferrocad_stage", name = "GizmoCommitEvent")]
#[derive(Clone)]
pub struct GizmoCommitEvent {
    pub(crate) inner: fs::GizmoCommitEvent,
}

#[pymethods]
impl GizmoCommitEvent {
    /// The targeted entity id.
    #[getter]
    fn target_id(&self) -> u32 {
        self.inner.target_id
    }

    /// The final transform to commit to the document (16 row-major floats).
    #[getter]
    fn total_transform(&self) -> Vec<f64> {
        self.inner.total_transform.values().to_vec()
    }

    fn __repr__(&self) -> String {
        format!("GizmoCommitEvent(target_id={})", self.inner.target_id)
    }
}

/// An interactive transform triad.
#[pyclass(module = "ferrocad_stage", name = "TransformGizmo")]
pub struct TransformGizmo {
    pub(crate) inner: fs::TransformGizmo,
}

#[pymethods]
impl TransformGizmo {
    /// A gizmo for `target_id`, centred at `origin` (default the world origin).
    #[new]
    #[pyo3(signature = (target_id, mode=GizmoMode::Universal, origin=(0.0, 0.0, 0.0)))]
    fn new(target_id: u32, mode: GizmoMode, origin: (f64, f64, f64)) -> Self {
        Self {
            inner: fs::TransformGizmo::new(target_id, vector(origin), mode.into()),
        }
    }

    /// The targeted entity id.
    #[getter]
    fn target_id(&self) -> u32 {
        self.inner.target_id()
    }

    /// The gizmo's origin.
    #[getter]
    fn origin(&self) -> (f64, f64, f64) {
        triple(self.inner.origin())
    }

    #[setter]
    fn set_origin(&mut self, origin: (f64, f64, f64)) {
        self.inner.set_origin(vector(origin));
    }

    /// The current mode.
    #[getter]
    fn mode(&self) -> GizmoMode {
        self.inner.mode().into()
    }

    #[setter]
    fn set_mode(&mut self, mode: GizmoMode) {
        self.inner.set_mode(mode.into());
    }

    /// The current world radius (`0` before the first `update`).
    #[getter]
    fn scale(&self) -> f64 {
        self.inner.scale()
    }

    /// The handle under the pointer, if any.
    #[getter]
    fn hovered(&self) -> Option<GizmoHandle> {
        self.inner.hovered().map(Into::into)
    }

    /// Whether a drag is in progress.
    #[getter]
    fn is_dragging(&self) -> bool {
        self.inner.is_dragging()
    }

    /// Highlight a handle (or clear). Returns whether the visuals changed.
    fn set_hovered(&mut self, handle: Option<GizmoHandle>) -> bool {
        self.inner.set_hovered(handle.map(Into::into))
    }

    /// Recompute the screen-invariant scale for `camera` (and rebuild if it changed).
    /// Returns whether the handle geometry was rebuilt.
    fn update(&mut self, camera: &Camera) -> bool {
        self.inner.update(&camera.inner)
    }

    /// Begin a drag on `handle` at the device-pixel `position`.
    fn begin_drag(&mut self, handle: GizmoHandle, camera: &Camera, position: (i32, i32)) -> bool {
        self.inner
            .begin_drag(handle.into(), &camera.inner, [position.0, position.1])
    }

    /// Advance the active drag to `position`, returning this tick's deltas.
    fn drag_to(&mut self, camera: &Camera, position: (i32, i32)) -> Option<GizmoDragEvent> {
        self.inner
            .drag_to(&camera.inner, [position.0, position.1])
            .map(|inner| GizmoDragEvent { inner })
    }

    /// End the active drag, returning the commit.
    fn end_drag(&mut self) -> Option<GizmoCommitEvent> {
        self.inner.end_drag().map(|inner| GizmoCommitEvent { inner })
    }

    /// Dispatch a pointer event: `Down` grabs the currently hovered handle, `Move`
    /// while dragging emits a [`GizmoDragEvent`], `Up` emits a [`GizmoCommitEvent`].
    /// Returns the emitted event, or `None`.
    fn handle_event(
        &mut self,
        py: Python<'_>,
        camera: &Camera,
        event: &PointerEvent,
    ) -> PyResult<Option<Py<PyAny>>> {
        match event.inner.kind {
            fs::PointerKind::Down => {
                if let Some(handle) = self.inner.hovered() {
                    self.inner
                        .begin_drag(handle, &camera.inner, event.inner.position);
                }
                Ok(None)
            }
            fs::PointerKind::Move => match self.inner.drag_to(&camera.inner, event.inner.position) {
                Some(drag) => Ok(Some(
                    Py::new(py, GizmoDragEvent { inner: drag })?.into_any(),
                )),
                None => Ok(None),
            },
            fs::PointerKind::Up => match self.inner.end_drag() {
                Some(commit) => Ok(Some(
                    Py::new(py, GizmoCommitEvent { inner: commit })?.into_any(),
                )),
                None => Ok(None),
            },
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "TransformGizmo(target_id={}, mode={:?})",
            self.inner.target_id(),
            self.inner.mode()
        )
    }
}

/// Register this module's classes.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<GizmoDragEvent>()?;
    m.add_class::<GizmoCommitEvent>()?;
    m.add_class::<TransformGizmo>()?;
    Ok(())
}

fn vector(v: (f64, f64, f64)) -> Vector3 {
    Vector3::new(v.0, v.1, v.2)
}

fn triple(v: Vector3) -> (f64, f64, f64) {
    (v.x, v.y, v.z)
}
