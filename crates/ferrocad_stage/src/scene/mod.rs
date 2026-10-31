//! The declarative scene: the [`Stage`] and the [`ShapeItem`]s it holds.
//!
//! A workbench describes geometry once as items; the stage compiles them straight to
//! draw batches. This is the high-level entry point to the crate's visuals; the
//! submodules stay public for a host that needs the item's fields directly.

pub mod shape;
pub mod stage;

pub use shape::{DrawMode, ShapeItem};
pub use stage::Stage;
