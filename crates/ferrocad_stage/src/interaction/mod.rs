//! Pointer input, tools, snapping and the manipulator model.
//!
//! A [`tool::Tool`] turns [`input::PointerEvent`]s into transient visuals, [`snap`]
//! resolves the pointer onto a geometric feature or the workplane grid, and
//! [`manipulator::Manipulator`] is a **workbench's own** draggable overlay (the edge
//! fillet arc, a BIM grip). [`declarative::DeclarativeManipulator`] is the data-only
//! form a Python mod can declare, and [`plane_points::PlanePointsTool`] is the
//! three-point workplane pick.

pub mod arc;
pub mod box_tool;
pub mod primitive;
pub mod circle;
pub mod declarative;
pub mod frame;
pub mod input;
pub mod manipulator;
pub mod plane_points;
pub mod polygon;
pub mod rectangle;
pub mod snap;
pub mod target;
pub mod tool;

pub use arc::ArcTool;
pub use box_tool::BoxTool;
pub use circle::CircleTool;
pub use declarative::{
    DeclarativeHandle, DeclarativeKind, DeclarativeManipulator, DeclarativeShape,
};
pub use frame::DragFrame;
pub use input::{MouseButton, PointerEvent, PointerKind, ViewProjection};
pub use manipulator::{CreationHost, DressUpKind, InteractionCommit, Manipulator, NumericField};
pub use plane_points::PlanePointsTool;
pub use polygon::PolygonTool;
pub use primitive::{PrimitiveKind, PrimitiveTool};
pub use rectangle::RectangleTool;
pub use snap::{
    Snap, SnapCandidate, SnapContext, SnapKind, nearest_feature, snap_to_geometry,
    snap_to_geometry_with,
};
pub use target::{HoverTarget, TargetKind};
pub use tool::{PolylineTool, TaggedPolyline, Tool, ToolField, ToolStatus};
