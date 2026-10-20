//! Pick results and pointer input for Python.
//!
//! [`Hit`] wraps the renderer-agnostic [`ferrocad_graphics::Hit`] the pick paths return
//! (object id, sub-element index, world point and normal), and [`PointerEvent`] wraps
//! [`ferrocad_stage::PointerEvent`] — the typed replacement for the `SoEvent`/
//! `SoEventCallback` bridge — so a Python [`Tool`](crate::Tool) can react to it.

use ferrocad_graphics as gfx;
use ferrocad_stage as fs;
use pyo3::prelude::*;

use crate::enums::{MouseButton, PointerKind};

/// A pick result (what a click landed on).
#[pyclass(module = "ferrocad_stage", name = "Hit")]
#[derive(Clone, Copy)]
pub struct Hit {
    pub(crate) inner: gfx::Hit,
}

impl Hit {
    /// Wrap a native hit.
    pub(crate) fn wrap(inner: gfx::Hit) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl Hit {
    /// The object that was hit (the packet's pick id).
    #[getter]
    fn entity_id(&self) -> u32 {
        self.inner.id.0
    }

    /// The topological sub-element id (`Vertex.part_idx`); `0` when untagged.
    #[getter]
    fn part_idx(&self) -> u32 {
        self.inner.part
    }

    /// The primitive index within the mesh (a triangle or segment ordinal), when the
    /// pick path knows it. The GPU pick resolves a pixel, so it reports `None`.
    #[getter]
    fn element(&self) -> Option<u32> {
        self.inner.element
    }

    /// The world-space intersection point.
    #[getter]
    fn point(&self) -> (f32, f32, f32) {
        triple(self.inner.point)
    }

    /// The world-space surface normal.
    #[getter]
    fn normal(&self) -> (f32, f32, f32) {
        triple(self.inner.normal)
    }

    /// The distance along the ray from its origin.
    #[getter]
    fn distance(&self) -> f64 {
        self.inner.distance
    }

    /// The topology of the hit: `"Triangles"`, `"Lines"` or `"Points"`.
    #[getter]
    fn topology(&self) -> &'static str {
        match self.inner.topology {
            gfx::Topology::Triangles => "Triangles",
            gfx::Topology::Lines => "Lines",
            gfx::Topology::Points => "Points",
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "Hit(entity_id={}, part_idx={}, point={:?})",
            self.inner.id.0,
            self.inner.part,
            triple(self.inner.point)
        )
    }
}

/// A structured pointer event (device pixels, bottom-left origin).
#[pyclass(module = "ferrocad_stage", name = "PointerEvent")]
#[derive(Clone, Copy)]
pub struct PointerEvent {
    pub(crate) inner: fs::PointerEvent,
    /// The resolved snap point, when the host supplied one.
    pub(crate) snap: Option<[f32; 3]>,
}

impl PointerEvent {
    /// Wrap a native event (`snap` is the resolved snap, if any).
    pub(crate) fn wrap(inner: fs::PointerEvent, snap: Option<fs::Snap>) -> Self {
        Self {
            inner,
            snap: snap.map(|snap| snap.point),
        }
    }

    /// Build one with an explicit kind, position, button and hit.
    fn build(
        kind: PointerKind,
        position: (i32, i32),
        button: Option<MouseButton>,
        hit: Option<Hit>,
    ) -> Self {
        Self {
            inner: fs::PointerEvent {
                kind: kind.into(),
                position: [position.0, position.1],
                button: button.map(Into::into),
                hit: hit.map(|hit| hit.inner),
            },
            snap: None,
        }
    }
}

#[pymethods]
impl PointerEvent {
    #[new]
    #[pyo3(signature = (kind, position, button=None, hit=None))]
    fn new(
        kind: PointerKind,
        position: (i32, i32),
        button: Option<MouseButton>,
        hit: Option<Hit>,
    ) -> Self {
        Self::build(kind, position, button, hit)
    }

    /// A pointer-move event at `position`.
    #[staticmethod]
    #[pyo3(name = "move")]
    fn move_at(position: (i32, i32)) -> Self {
        Self::build(PointerKind::Move, position, None, None)
    }

    /// A button-press event at `position`.
    #[staticmethod]
    fn down(position: (i32, i32), button: MouseButton) -> Self {
        Self::build(PointerKind::Down, position, Some(button), None)
    }

    /// A button-release event at `position`.
    #[staticmethod]
    fn up(position: (i32, i32), button: MouseButton) -> Self {
        Self::build(PointerKind::Up, position, Some(button), None)
    }

    /// Which event this is.
    #[getter]
    fn kind(&self) -> PointerKind {
        self.inner.kind.into()
    }

    /// The device-pixel position (bottom-left origin).
    #[getter]
    fn position(&self) -> (i32, i32) {
        (self.inner.position[0], self.inner.position[1])
    }

    /// The button, for a `Down`/`Up` event.
    #[getter]
    fn button(&self) -> Option<MouseButton> {
        self.inner.button.map(Into::into)
    }

    /// The pick under the pointer, if it hit geometry.
    #[getter]
    fn hit(&self) -> Option<Hit> {
        self.inner.hit.map(Hit::wrap)
    }

    /// The resolved snap point, if the host supplied one.
    #[getter]
    fn snap(&self) -> Option<(f32, f32, f32)> {
        self.snap.map(triple)
    }

    /// The world-space point under the pointer, if it hit geometry.
    fn world(&self) -> Option<(f32, f32, f32)> {
        self.inner.world().map(triple)
    }

    /// Return a copy carrying `snap` (set by a host before dispatch).
    fn with_snap(&self, snap: Option<(f32, f32, f32)>) -> Self {
        Self {
            inner: self.inner,
            snap: snap.map(|(x, y, z)| [x, y, z]),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "PointerEvent(kind={:?}, position={:?}, button={:?})",
            self.inner.kind, self.inner.position, self.inner.button
        )
    }
}

/// Register this module's classes.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Hit>()?;
    m.add_class::<PointerEvent>()?;
    Ok(())
}

fn triple(v: [f32; 3]) -> (f32, f32, f32) {
    (v[0], v[1], v[2])
}
