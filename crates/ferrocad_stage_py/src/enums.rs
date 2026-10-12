//! Python mirrors of the `ferrocad_stage` enums.
//!
//! `ferrocad_stage`'s enums are plain Rust types (`#[pyclass]` cannot be applied to a
//! foreign type), so each is mirrored here as a fieldless `#[pyclass]` enum — its
//! variants become class attributes, so Python writes `stage.DrawMode.Shaded` — with
//! `From` conversions both ways.

use ferrocad_stage as fs;
use pyo3::prelude::*;

/// How a [`ShapeItem`](crate::ShapeItem) draws (`ferrocad_stage::DrawMode`).
#[pyclass(eq, module = "ferrocad_stage", name = "DrawMode")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DrawMode {
    /// Shaded surfaces only.
    Shaded,
    /// Edges only.
    Wireframe,
    /// Shaded surfaces with an edge wireframe on top.
    ShadedWithEdges,
}

impl From<DrawMode> for fs::DrawMode {
    fn from(mode: DrawMode) -> Self {
        match mode {
            DrawMode::Shaded => fs::DrawMode::Shaded,
            DrawMode::Wireframe => fs::DrawMode::Wireframe,
            DrawMode::ShadedWithEdges => fs::DrawMode::ShadedWithEdges,
        }
    }
}

impl From<fs::DrawMode> for DrawMode {
    fn from(mode: fs::DrawMode) -> Self {
        match mode {
            fs::DrawMode::Shaded => DrawMode::Shaded,
            fs::DrawMode::Wireframe => DrawMode::Wireframe,
            fs::DrawMode::ShadedWithEdges => DrawMode::ShadedWithEdges,
        }
    }
}

/// A mouse button (`ferrocad_stage::MouseButton`).
#[pyclass(eq, module = "ferrocad_stage", name = "MouseButton")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MouseButton {
    /// The primary button.
    Left,
    /// The middle button.
    Middle,
    /// The secondary button.
    Right,
}

impl From<MouseButton> for fs::MouseButton {
    fn from(button: MouseButton) -> Self {
        match button {
            MouseButton::Left => fs::MouseButton::Left,
            MouseButton::Middle => fs::MouseButton::Middle,
            MouseButton::Right => fs::MouseButton::Right,
        }
    }
}

impl From<fs::MouseButton> for MouseButton {
    fn from(button: fs::MouseButton) -> Self {
        match button {
            fs::MouseButton::Left => MouseButton::Left,
            fs::MouseButton::Middle => MouseButton::Middle,
            fs::MouseButton::Right => MouseButton::Right,
        }
    }
}

/// Which pointer event occurred (`ferrocad_stage::PointerKind`).
#[pyclass(eq, module = "ferrocad_stage", name = "PointerKind")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PointerKind {
    /// The pointer moved.
    Move,
    /// A button was pressed.
    Down,
    /// A button was released.
    Up,
}

impl From<PointerKind> for fs::PointerKind {
    fn from(kind: PointerKind) -> Self {
        match kind {
            PointerKind::Move => fs::PointerKind::Move,
            PointerKind::Down => fs::PointerKind::Down,
            PointerKind::Up => fs::PointerKind::Up,
        }
    }
}

impl From<fs::PointerKind> for PointerKind {
    fn from(kind: fs::PointerKind) -> Self {
        match kind {
            fs::PointerKind::Move => PointerKind::Move,
            fs::PointerKind::Down => PointerKind::Down,
            fs::PointerKind::Up => PointerKind::Up,
        }
    }
}

/// Which handles a [`TransformGizmo`](crate::TransformGizmo) shows
/// (`ferrocad_stage::GizmoMode`).
#[pyclass(eq, module = "ferrocad_stage", name = "GizmoMode")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GizmoMode {
    /// Translation axes and planar quads.
    Translate,
    /// Rotation rings.
    Rotate,
    /// Everything (translation + rotation).
    Universal,
}

impl From<GizmoMode> for fs::GizmoMode {
    fn from(mode: GizmoMode) -> Self {
        match mode {
            GizmoMode::Translate => fs::GizmoMode::Translate,
            GizmoMode::Rotate => fs::GizmoMode::Rotate,
            GizmoMode::Universal => fs::GizmoMode::Universal,
        }
    }
}

impl From<fs::GizmoMode> for GizmoMode {
    fn from(mode: fs::GizmoMode) -> Self {
        match mode {
            fs::GizmoMode::Translate => GizmoMode::Translate,
            fs::GizmoMode::Rotate => GizmoMode::Rotate,
            fs::GizmoMode::Universal => GizmoMode::Universal,
        }
    }
}

/// A gizmo sub-handle, a degree of freedom (`ferrocad_stage::GizmoHandle`).
#[pyclass(eq, module = "ferrocad_stage", name = "GizmoHandle")]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GizmoHandle {
    /// Translate along X.
    AxisX,
    /// Translate along Y.
    AxisY,
    /// Translate along Z.
    AxisZ,
    /// Translate in the XY plane.
    PlaneXY,
    /// Translate in the XZ plane.
    PlaneXZ,
    /// Translate in the YZ plane.
    PlaneYZ,
    /// Rotate about X.
    RotateX,
    /// Rotate about Y.
    RotateY,
    /// Rotate about Z.
    RotateZ,
}

impl From<GizmoHandle> for fs::GizmoHandle {
    fn from(handle: GizmoHandle) -> Self {
        match handle {
            GizmoHandle::AxisX => fs::GizmoHandle::AxisX,
            GizmoHandle::AxisY => fs::GizmoHandle::AxisY,
            GizmoHandle::AxisZ => fs::GizmoHandle::AxisZ,
            GizmoHandle::PlaneXY => fs::GizmoHandle::PlaneXY,
            GizmoHandle::PlaneXZ => fs::GizmoHandle::PlaneXZ,
            GizmoHandle::PlaneYZ => fs::GizmoHandle::PlaneYZ,
            GizmoHandle::RotateX => fs::GizmoHandle::RotateX,
            GizmoHandle::RotateY => fs::GizmoHandle::RotateY,
            GizmoHandle::RotateZ => fs::GizmoHandle::RotateZ,
        }
    }
}

impl From<fs::GizmoHandle> for GizmoHandle {
    fn from(handle: fs::GizmoHandle) -> Self {
        match handle {
            fs::GizmoHandle::AxisX => GizmoHandle::AxisX,
            fs::GizmoHandle::AxisY => GizmoHandle::AxisY,
            fs::GizmoHandle::AxisZ => GizmoHandle::AxisZ,
            fs::GizmoHandle::PlaneXY => GizmoHandle::PlaneXY,
            fs::GizmoHandle::PlaneXZ => GizmoHandle::PlaneXZ,
            fs::GizmoHandle::PlaneYZ => GizmoHandle::PlaneYZ,
            fs::GizmoHandle::RotateX => GizmoHandle::RotateX,
            fs::GizmoHandle::RotateY => GizmoHandle::RotateY,
            fs::GizmoHandle::RotateZ => GizmoHandle::RotateZ,
        }
    }
}

/// Register every enum on the module.
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<DrawMode>()?;
    m.add_class::<MouseButton>()?;
    m.add_class::<PointerKind>()?;
    m.add_class::<GizmoMode>()?;
    m.add_class::<GizmoHandle>()?;
    Ok(())
}
