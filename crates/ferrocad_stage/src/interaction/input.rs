//! Structured pointer input delivered to a [`Tool`](crate::Tool).
//!
//! This is the modern counterpart of the `SoEvent`/`SoEventCallback` bridge: a typed
//! event carrying the device-pixel position, the CPU pick result under the pointer and
//! the frame's projection. Positions are **device pixels** (bottom-left origin, Coin's
//! convention), like the rest of the stack below the host.

use ferrocad_graphics::{Camera, Hit};
use ferrocad_types::{Matrix4, Vector3};

/// Which pointer event occurred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerKind {
    /// The pointer moved.
    Move,
    /// A button was pressed.
    Down,
    /// A button was released.
    Up,
}

/// A mouse button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    /// The primary button.
    Left,
    /// The middle button.
    Middle,
    /// The secondary button.
    Right,
}

/// A pointer event handed to a [`Tool`](crate::Tool).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointerEvent {
    /// Which event this is.
    pub kind: PointerKind,
    /// The pointer position, in **device** pixels (bottom-left origin).
    pub position: [i32; 2],
    /// The button, for [`Down`](PointerKind::Down)/[`Up`](PointerKind::Up).
    pub button: Option<MouseButton>,
    /// The CPU pick result under the pointer, if it hit geometry.
    pub hit: Option<Hit>,
    /// The frame's world → device projection, so a tool can place its **own** geometry on
    /// screen (a vertex hit test in screen space, not a world-space radius). `None` when
    /// the host has no camera to offer (a headless test); a tool must then fall back to
    /// not hit-testing its own geometry.
    pub projection: Option<ViewProjection>,
}

impl PointerEvent {
    /// The world-space point under the pointer, if it hit geometry.
    pub fn world(&self) -> Option<[f32; 3]> {
        self.hit.as_ref().map(|hit| hit.point)
    }
}

/// A frame's world → device-pixel projection, for a tool that must hit-test its own
/// geometry in screen space.
///
/// A tool sees device-pixel pointer positions but no camera. Carrying the combined
/// view-projection (and the viewport size) on the event is enough for it to project its
/// own vertices itself — no host-side per-tool hit test, and no world-space radius, which
/// would only be right at one zoom level.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewProjection {
    matrix: Matrix4,
    device_size: [u32; 2],
}

impl ViewProjection {
    /// From an explicit world → clip matrix and the device-pixel viewport size.
    pub fn new(matrix: Matrix4, device_size: [u32; 2]) -> Self {
        Self {
            matrix,
            device_size,
        }
    }

    /// The projection of a frame's camera.
    pub fn from_camera(camera: &Camera) -> Self {
        Self::new(camera.view_projection(), camera.viewport.device_size())
    }

    /// The combined world → clip matrix.
    pub fn matrix(&self) -> Matrix4 {
        self.matrix
    }

    /// The viewport size, in device pixels.
    pub fn device_size(&self) -> [u32; 2] {
        self.device_size
    }

    /// Project a world point to device pixels (bottom-left origin). `None` when the point
    /// is on or behind the camera's `w = 0` plane.
    pub fn project(&self, point: [f32; 3]) -> Option<[f64; 2]> {
        crate::screen::project_device_with(
            &self.matrix,
            self.device_size,
            Vector3::new(
                f64::from(point[0]),
                f64::from(point[1]),
                f64::from(point[2]),
            ),
        )
    }

    /// The distance, in device pixels, from `device` to the projected `point`.
    pub fn pixels_to(&self, point: [f32; 3], device: [i32; 2]) -> Option<f64> {
        let projected = self.project(point)?;
        Some((projected[0] - f64::from(device[0])).hypot(projected[1] - f64::from(device[1])))
    }
}
