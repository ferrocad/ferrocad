//! # FerroCAD Stage (`ferrocad_stage`)
//!
//! The **modern** presentation side of the FerroCAD visuals: a declarative CAD
//! framework that compiles *straight* to the shared draw-batch vocabulary, replacing
//! the imperative `pivy.coin` node tree for new workbenches.
//!
//! A workbench declares high-level visual items (a [`ShapeItem`] with tessellation and
//! display attributes) on a [`Stage`]; [`Stage::emit_batches`] decomposes each into
//! [`DrawPacket`](ferrocad_graphics::DrawPacket)s. There is **no intermediate scene
//! graph** — no node arena, no traversal — so a stage is cheap to rebuild each frame.
//!
//! ```text
//! Stage ──emit_batches──▶ RenderQueue { DrawPacket … } ──DrawBatch──▶ renderer
//!                        ▲
//!       ferrocad_inventor::compile ──┘   (the legacy pivy.coin path emits the same)
//! ```
//!
//! Because both stacks emit the **same** batch vocabulary, a renderer draws them in one
//! pass against one depth buffer — no visual layering artifacts (see
//! [`ferrocad_graphics`]).
//!
//! This crate is pure Rust: no `pyo3`, no `wgpu`. The Python surface (`ferrocad.stage`)
//! and the interactive gizmos are later slices; see `docs/developer/viewport/stage.md`.
//!
//! Beyond shapes, the crate carries the interactive layer: [`PointerEvent`]/
//! [`PointerKind`]/[`MouseButton`] (the structured replacement for `SoEventCallback`),
//! [`snap_to_geometry`] ([`Snap`]/[`SnapKind`]), the [`Tool`] trait with a reference
//! [`PolylineTool`], a [`TransformGizmo`] (the `SoDragger` replacement) with its
//! constraint-projection math and screen-space-invariant scale, and the
//! [`Manipulator`] trait with its [`InteractionCommit`]. A tool turns pointer events
//! into transient visuals; a gizmo turns a handle drag into a delta transform the host
//! previews on the target item without touching the document; a manipulator is a
//! **workbench's own** draggable overlay (the edge fillet arc is one, in
//! `ferrocad_part_gui`), and it commits neutral data the workbench interprets.
//! A [`TransformSession`] is the pending value of one cumulative transform gesture — the
//! datum, the delta from it and the mode — held by the shell and rendered by the viewport
//! instead of being written onto the display item.
//! Everything in this crate is workbench-agnostic: it names no kernel, no document and
//! no module.
//!
//! A transform gizmo is one of three [`GizmoKind`]s (`translate`, `rotate` or the
//! composed `universal`). A registry offers a [`GizmoSet`] — an ordered list whose order
//! **is** the hit priority — and a session verb narrows it to one kind. The per-kind
//! constructors ([`TransformGizmo::translate`] and friends) share the frame, hit and
//! solver math; only the handle set differs.
//!
//! # Module map
//!
//! The crate is grouped by responsibility; every submodule stays public, so a host
//! can use the high-level interface or reach for one piece directly.
//!
//! ```text
//! scene/       Stage, ShapeItem, DrawMode          (what is drawn)
//! render/      grid, axis, feedback, screen        (producers and pixel math)
//! interaction/ input, tool, snap, manipulator,     (pointer, picking, overlays)
//!              plane_points, declarative
//! transform/   gizmo, session, protractor,         (the interactive transform)
//!              measure_tape
//! workplane/   workplane, stem                     (planar frames)
//! ```
//!
//! The older flat module paths (`ferrocad_stage::gizmo`, `::screen`, …) are kept as
//! re-exports of the same modules, so existing callers keep working.
//!
//! # Example
//!
//! ```
//! use ferrocad_stage::{DrawMode, ShapeItem, Stage};
//!
//! // A triangle with its three edges, shaded with edges.
//! let item = ShapeItem::new(7)
//!     .with_draw_mode(DrawMode::ShadedWithEdges)
//!     .with_shape(
//!         vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
//!         vec![],
//!         vec![0, 1, 2],
//!         vec![0, 1, 1, 2, 2, 0],
//!     );
//!
//! let queue = Stage::new().with_item(item).to_queue();
//! // One shaded surface packet and one wireframe packet.
//! assert_eq!(queue.len(), 2);
//! ```

pub mod interaction;
pub mod render;
pub mod scene;
pub mod transform;
// The published view (camera, stage and previews a host last drew). It lives in the
// pure stage crate so the platform-neutral shell can publish it without linking
// CPython; `ferrocad_stage_py` wraps it for the `FreeCADGui` facade.
pub mod view;
pub mod workplane;

// The flat module paths predate the folder hierarchy and remain part of the stable
// library surface: `ferrocad_stage::gizmo::…` and
// `ferrocad_stage::transform::gizmo::…` name the same module.
pub use interaction::{declarative, input, plane_points, snap, tool};
pub use render::{axis, feedback, grid, screen};
pub use scene::{shape, stage};
pub use transform::{gizmo, measure_tape, protractor, session};
pub use workplane::stem as workplane_stem;

pub use gizmo::{
    DEFAULT_GHOST_ALPHA, DEFAULT_HALO_WIDTH, DEFAULT_HIT_RADIUS_PX, DEFAULT_HOVER_SCALE,
    GIZMO_BASE_ID, GizmoAxis, GizmoCommitEvent, GizmoConstraint, GizmoDragEvent, GizmoHandle,
    GizmoKind, GizmoMode, GizmoModifiers, GizmoSet, GizmoStyle, GizmoTarget, GizmoValue,
    HandleKind, MAX_GHOST_ALPHA, MIN_HIT_RADIUS_PX, ManipulatorFrame, NATIVE_BACKEND,
    OUTLINE_COLOR, RingSweep, SKIN_BACKEND, TRANSFORM_ID, TransformGizmo,
};
pub use axis::AxisStyle;
pub use declarative::{DeclarativeHandle, DeclarativeKind, DeclarativeManipulator, DeclarativeShape};
pub use grid::{GridGeometry, GridSpec, GridStyle, Ground, MinorStyle};
pub use input::{MouseButton, PointerEvent, PointerKind, ViewProjection};
pub use interaction::{InteractionCommit, Manipulator};
pub use plane_points::PlanePointsTool;
pub use screen::{HIT_PADDING_PIXELS, HIT_SLOP, HIT_TOLERANCE_PIXELS, STROKE_PIXELS, scale_for};
pub use session::{TransformSession, TransformTape};
pub use shape::{DrawMode, ShapeItem};
/// The GPU-side visual tokens (also available as `ferrocad_graphics::theme`).
pub use ferrocad_graphics::{ShaderProgram, theme};
pub use snap::{
    Snap, SnapCandidate, SnapContext, SnapKind, nearest_feature, snap_to_geometry,
    snap_to_geometry_with,
};
pub use stage::Stage;
pub use tool::{PolylineTool, Tool, ToolStatus};
pub use workplane::{FaceFrame, StandardPlane, Workplane, WorkplaneKind};
pub use workplane_stem::{
    STEM_PIXELS, WORKPLANE_ENTITY, WORKPLANE_OFFSET_ACTION, WORKPLANE_OFFSET_ID,
    WORKPLANE_STEM_PICK_ID, WorkplaneLive, WorkplaneStem,
};
