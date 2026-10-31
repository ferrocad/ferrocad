//! Pointer input, tools, snapping and the manipulator model.
//!
//! A [`tool::Tool`] turns [`input::PointerEvent`]s into transient visuals, [`snap`]
//! resolves the pointer onto a geometric feature or the workplane grid, and
//! [`manipulator::Manipulator`] is a **workbench's own** draggable overlay (the edge
//! fillet arc, a BIM grip). [`declarative::DeclarativeManipulator`] is the data-only
//! form a Python mod can declare, and [`plane_points::PlanePointsTool`] is the
//! three-point workplane pick.

pub mod declarative;
pub mod input;
pub mod manipulator;
pub mod plane_points;
pub mod snap;
pub mod tool;

pub use declarative::{
    DeclarativeHandle, DeclarativeKind, DeclarativeManipulator, DeclarativeShape,
};
pub use input::{MouseButton, PointerEvent, PointerKind, ViewProjection};
pub use manipulator::{InteractionCommit, Manipulator};
pub use plane_points::PlanePointsTool;
pub use snap::{
    Snap, SnapCandidate, SnapContext, SnapKind, nearest_feature, snap_to_geometry,
    snap_to_geometry_with,
};
pub use tool::{PolylineTool, Tool, ToolStatus};
