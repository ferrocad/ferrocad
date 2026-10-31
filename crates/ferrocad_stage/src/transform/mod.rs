//! The interactive transform: the gizmo, the pending session and their 3-D readouts.
//!
//! [`gizmo::TransformGizmo`] turns a handle drag into a delta transform the host
//! previews on the target item without touching the document;
//! [`session::TransformSession`] is that pending value (datum, delta and mode); and
//! [`protractor`] / [`measure_tape`] draw the angular and linear readouts from the
//! same snapshot, so the gesture and its numbers cannot disagree.

pub mod gizmo;
pub mod measure_tape;
pub mod protractor;
pub mod session;

pub use gizmo::{
    DEFAULT_GHOST_ALPHA, DEFAULT_HALO_WIDTH, DEFAULT_HIT_RADIUS_PX, DEFAULT_HOVER_SCALE,
    GIZMO_BASE_ID, GizmoAxis, GizmoCommitEvent, GizmoConstraint, GizmoDragEvent, GizmoHandle,
    GizmoKind, GizmoMode, GizmoModifiers, GizmoSet, GizmoStyle, GizmoTarget, GizmoValue,
    HandleKind, MAX_GHOST_ALPHA, MIN_HIT_RADIUS_PX, ManipulatorFrame, NATIVE_BACKEND,
    OUTLINE_COLOR, RingSweep, SKIN_BACKEND, TRANSFORM_ID, TransformGizmo,
};
pub use session::{TransformSession, TransformTape};
