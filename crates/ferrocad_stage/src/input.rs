//! Structured pointer input delivered to a [`Tool`](crate::Tool).
//!
//! This is the modern counterpart of the `SoEvent`/`SoEventCallback` bridge: a typed
//! event carrying the device-pixel position and the CPU pick result under the pointer.
//! Positions are **device pixels** (bottom-left origin, Coin's convention), like the
//! rest of the stack below the host.

use ferrocad_inventor::Hit;

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
}

impl PointerEvent {
    /// The world-space point under the pointer, if it hit geometry.
    pub fn world(&self) -> Option<[f32; 3]> {
        self.hit.as_ref().map(|hit| hit.point)
    }
}
