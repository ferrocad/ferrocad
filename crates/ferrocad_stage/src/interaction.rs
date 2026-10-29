//! Workbench manipulators: the generic interaction model.
//!
//! The stage owns the manipulators **every** workbench shares — the [`Tool`] (a
//! rubber-band builder) and the [`TransformGizmo`]. A workbench-specific
//! manipulator (an edge fillet arc, a wall-thickness grip, a rebar hook) does not
//! belong here: it lives in that workbench's presentation crate and implements
//! [`Manipulator`]. The viewport holds an ordered list of them, so adding an
//! interaction never touches the viewport, the shell or another workbench.
//!
//! ```text
//! selection ──▶ factory (the edition) ──▶ Box<dyn Manipulator> ──▶ viewport
//!                                          │ pointer drag
//!                                          ▼
//!                              InteractionCommit (neutral data)
//!                                          │
//!                                          ▼
//!                     the edition's handler / the app script
//! ```
//!
//! A commit is **data**, not a typed Rust variant: it names the manipulator, the
//! target and the operation, carries the numbers in the manipulator's documented
//! order, and carries the display readout. That is what lets `ferrocad_viewport`
//! and `ferrocad_gpui` route any workbench's interaction without naming one.
//!
//! [`Tool`]: crate::Tool
//! [`TransformGizmo`]: crate::TransformGizmo

use ferrocad_graphics::Camera;
use ferrocad_types::Vector3;

use crate::shape::ShapeItem;

/// A value a [`Manipulator`] commits to the document.
///
/// The viewport and the shell treat this as opaque data: they route it by
/// [`manipulator`](Self::manipulator) to whoever registered that manipulator, and
/// they never need to know what [`values`](Self::values) mean. The manipulator
/// documents their order (a fillet is `[radius]`, a chamfer is
/// `[first, second]`).
#[derive(Clone, Debug, PartialEq)]
pub struct InteractionCommit {
    /// The manipulator's registry id (`"dress_up"`).
    ///
    /// Owned, not a literal: a Rust workbench names a constant, while a declarative
    /// (mod-supplied) manipulator carries an id built at run time.
    pub manipulator: String,
    /// The entity the manipulation applies to.
    pub target: u32,
    /// The named sub-element it applies to, when it acts on one (`Edge3`).
    pub element: Option<String>,
    /// The operation within the manipulator (`"fillet"`, `"chamfer"`).
    pub action: String,
    /// The committed numbers, in the manipulator's documented order.
    pub values: Vec<f64>,
    /// The human readout, for the status line (`R 3.50 mm`).
    pub readout: String,
    /// Whether this commit **creates** a new document object instead of editing
    /// [`target`](Self::target).
    ///
    /// A manipulator edits what it grabbed, so it always has a target. A tool that
    /// finishes a shape has none yet — the host must route such a commit to the
    /// workbench without resolving an entity first (see [`InteractionCommit::creating`]).
    pub creates: bool,
}

impl InteractionCommit {
    /// A commit for `action` on `target`.
    ///
    /// `manipulator` accepts a `&'static str` as before; the commit stores an owned
    /// [`String`] so a declarative handle can supply its id at run time.
    pub fn new(manipulator: impl Into<String>, target: u32, action: impl Into<String>) -> Self {
        Self {
            manipulator: manipulator.into(),
            target,
            element: None,
            action: action.into(),
            values: Vec::new(),
            readout: String::new(),
            creates: false,
        }
    }

    /// Mark this commit as creating a new object rather than editing `target`.
    ///
    /// The host routes it to the workbench's `interaction` hook without looking the
    /// entity up; the workbench owns the creation (and its transaction).
    pub fn creating(mut self) -> Self {
        self.creates = true;
        self
    }

    /// The builder form of the sub-element.
    pub fn with_element(mut self, element: impl Into<String>) -> Self {
        self.element = Some(element.into());
        self
    }

    /// The builder form of the numbers.
    pub fn with_values(mut self, values: impl IntoIterator<Item = f64>) -> Self {
        self.values = values.into_iter().collect();
        self
    }

    /// The builder form of the readout.
    pub fn with_readout(mut self, readout: impl Into<String>) -> Self {
        self.readout = readout.into();
        self
    }
}

/// A workbench manipulator: an overlay the user drags to produce a document edit.
///
/// This is the second extension point beside [`Tool`](crate::Tool). A tool reacts
/// to pointer events and draws a preview; a manipulator owns a **grab target**, a
/// **drag** and a **commit**. The viewport dispatches pointer events to the
/// manipulators in reverse registration order (the most recently armed one wins),
/// then to the gizmo, the tool and the scene.
///
/// Implementors live in a workbench's presentation crate (for example
/// `ferrocad_part_gui`), never in this crate and never in the viewport.
pub trait Manipulator {
    /// The registry id, used to replace or remove the manipulator and to route its
    /// commit back to the workbench that registered it.
    ///
    /// A Rust workbench returns a literal; a declarative (mod-supplied) manipulator
    /// returns its owned id.
    fn id(&self) -> &str;

    /// The overlay items, in draw order. They are drawn on top with depth off.
    fn items(&self) -> &[ShapeItem];

    /// Whether `device` is on a grab target. An unclaimed press falls through to
    /// the gizmo, the tool and the scene.
    fn hit(&self, camera: &Camera, device: [i32; 2]) -> bool;

    /// Begin a drag at `device`, if it hits. Returns whether the drag started.
    fn drag_begin(&mut self, camera: &Camera, device: [i32; 2]) -> bool;

    /// Advance the drag. Returns whether the visuals or the value changed.
    fn drag_update(&mut self, camera: &Camera, device: [i32; 2]) -> bool;

    /// End the drag, returning the value to commit (if any).
    fn drag_end(&mut self) -> Option<InteractionCommit>;

    /// Cancel the drag, restoring the value it started from. Returns whether a drag
    /// was active.
    fn drag_cancel(&mut self) -> bool;

    /// Whether a drag is in progress. An active drag consumes every pointer event.
    fn is_dragging(&self) -> bool;

    /// Highlight (or un-highlight) the grab target. Returns whether the visuals
    /// changed.
    fn set_hovered(&mut self, hovered: bool) -> bool;

    /// Rebuild screen-space geometry after a camera change. Returns whether the
    /// visuals changed.
    fn refresh(&mut self, camera: &Camera) -> bool;

    /// Apply the workspace gizmo style. Returns whether the visuals changed.
    fn set_style(&mut self, _style: &crate::gizmo::GizmoStyle) -> bool {
        false
    }

    /// The readout to display beside the overlay (`R 3.50 mm`), if any.
    fn readout(&self) -> Option<String> {
        None
    }

    /// The world anchor this overlay marks, when it has one.
    ///
    /// A host uses it to keep the handle attached to an object it is previewing:
    /// moving the object must move the grip that sits on its top face too.
    fn anchor(&self) -> Option<Vector3> {
        None
    }

    /// Move the overlay's world anchor. Returns whether the visuals changed.
    ///
    /// The next [`refresh`](Self::refresh) rebuilds the screen-invariant geometry at
    /// the new anchor.
    fn set_anchor(&mut self, _anchor: Vector3) -> bool {
        false
    }

    /// Where the readout sits, in device pixels.
    fn readout_anchor(&self, _camera: &Camera) -> Option<[f64; 2]> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commit_carries_the_manipulator_op_target_and_values() {
        let commit = InteractionCommit::new("dress_up", 7, "chamfer")
            .with_element("Edge3")
            .with_values([2.0, 1.0])
            .with_readout("2.00 × 1.00 mm");

        assert_eq!(commit.manipulator, "dress_up");
        assert_eq!(commit.target, 7);
        assert_eq!(commit.element.as_deref(), Some("Edge3"));
        assert_eq!(commit.action, "chamfer");
        assert_eq!(commit.values, vec![2.0, 1.0]);
        assert_eq!(commit.readout, "2.00 × 1.00 mm");
    }
}
