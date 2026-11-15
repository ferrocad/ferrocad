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
//! and `ferrocad_shell_python` route any workbench's interaction without naming one.
//!
//! [`Tool`]: crate::Tool
//! [`TransformGizmo`]: crate::TransformGizmo

use ferrocad_graphics::Camera;
use ferrocad_types::Vector3;

use crate::shape::ShapeItem;

/// One number of a manipulator's pending value, as the host's numeric field shows it.
///
/// A manipulator's value is whatever its commit carries (a push/pull distance, a fillet
/// radius, a chamfer's two distances). The host does not know what those numbers mean, so
/// the manipulator describes them: this is the typed twin of a drag, the same way
/// [`InteractionCommit`] is the data twin of the release.
#[derive(Clone, Debug, PartialEq)]
pub struct NumericField {
    /// The short label the row prints before the input (`D`, `R`, `D1`).
    pub label: String,
    /// The unit after the number (`mm`), empty for none.
    pub unit: String,
    /// The value to show, in the manipulator's own units.
    pub value: f64,
    /// How many decimals to show (and round a typed value to).
    pub decimals: usize,
    /// The **hard** lower bound: the manipulator keeps the value at or above it.
    ///
    /// `None` is unbounded. A hard bound is one the operation cannot survive, or one the
    /// geometry makes impossible — not a preference.
    pub min: Option<f64>,
    /// The **hard** upper bound, on the same terms as [`min`](Self::min).
    pub max: Option<f64>,
    /// A note about the geometry the value acts on (`body 8.00 mm thick`), printed muted
    /// after the row.
    ///
    /// This is a **hint**, not a bound: the value stays legal beyond it, so the row must
    /// not clamp to it. It exists so a depth or a radius can be chosen with the model in
    /// view instead of by trial recompute.
    pub hint: Option<String>,
}

impl NumericField {
    /// A millimetre field labelled `label`.
    pub fn millimetres(label: impl Into<String>, value: f64) -> Self {
        Self {
            label: label.into(),
            unit: "mm".to_string(),
            value,
            decimals: 2,
            min: None,
            max: None,
            hint: None,
        }
    }

    /// Bound the value to `min..=max` (either end open).
    pub fn bounded(mut self, min: Option<f64>, max: Option<f64>) -> Self {
        self.min = min;
        self.max = max;
        self
    }

    /// Show (and round a typed value to) `decimals` places.
    pub fn decimals(mut self, decimals: usize) -> Self {
        self.decimals = decimals;
        self
    }

    /// Attach a geometry note (see [`hint`](Self::hint)).
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

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
    /// The body a **creating** tool anchored on, when it bound one.
    ///
    /// A drawing tool can start on the workplane or on a body: pressing a planar
    /// `Face6` binds that face's plane *and* names the face, so the workbench can
    /// **imprint** the drawn profile onto it instead of adding a separate object. The
    /// shell resolves [`CreationHost::entity`] to the document name the script receives,
    /// and reports [`CreationHost::element`] as the element, so the two outcomes are one
    /// call apart.
    pub host: Option<CreationHost>,
}

/// The body (and sub-element) a creating tool anchored on.
///
/// The stage knows entities, not document names: this is the neutral record the shell
/// turns into `("Box", "Face6")` for the app script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreationHost {
    /// The body's stage entity id.
    pub entity: u32,
    /// The sub-element that was bound (`Face6`), when the anchor was one.
    pub element: Option<String>,
    /// The document object the shell resolved [`entity`](Self::entity) to.
    ///
    /// The stage never sets this: [`entity`](Self::entity) is the stable handle, and the
    /// name belongs to the document half. The shell writes it in when it hands the commit
    /// to a backend, so the app script receives `("Box", "Face6")` without the stage
    /// learning any document vocabulary.
    pub object: Option<String>,
}

impl CreationHost {
    /// A host on the whole body (`element` is `None`).
    pub fn body(entity: u32) -> Self {
        Self {
            entity,
            element: None,
            object: None,
        }
    }

    /// A host on one of the body's sub-elements.
    pub fn element(entity: u32, element: impl Into<String>) -> Self {
        Self {
            entity,
            element: Some(element.into()),
            object: None,
        }
    }

    /// The shell's resolution of [`entity`](Self::entity) to a document object name.
    pub fn resolved(mut self, object: impl Into<String>) -> Self {
        self.object = Some(object.into());
        self
    }

    /// The element name, when the anchor was one.
    pub fn element_name(&self) -> Option<&str> {
        self.element.as_deref()
    }
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
            host: None,
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

    /// Name the body a creating commit anchored on.
    pub fn with_host(mut self, host: CreationHost) -> Self {
        self.host = Some(host);
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
/// **drag**, an optional display-only **preview** and a **commit**. The viewport
/// dispatches pointer events to the manipulators in reverse registration order (the most
/// recently armed one wins), then to the gizmo, the tool and the scene.
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

    /// Display-only items for the pending edit while a drag is active.
    ///
    /// This is the manipulator's half of the same contract as `FreeCADGui.Preview`:
    /// transient geometry drawn over the scene, never picked and never written to the
    /// document. The viewport shows them while [`is_dragging`](Self::is_dragging) is true
    /// **or** [`is_pending`](Self::is_pending) is — a held commit keeps its preview, so
    /// the user can still see (and re-drag) the value before deciding. A cancel removes
    /// it without an undo step. The default is empty: a manipulator whose pending
    /// property cannot be shown honestly draws only its handle.
    fn preview_items(&self) -> &[ShapeItem] {
        &[]
    }

    /// Whether the manipulator's value is held for the host's decision.
    ///
    /// A release is not a cancel: the host may hold the commit behind an `✗`/`✓` pair
    /// while the user reconsiders. The preview stays visible in that state, and a new
    /// drag continues from the held value, so the pending edit can still be seen and
    /// adjusted. The default is `false` — a host that applies a commit immediately never
    /// enters the state, and a manipulator keeps no value between gestures.
    fn is_pending(&self) -> bool {
        false
    }

    /// The typed twin of the pending value: one field per number the commit carries, in
    /// the commit's order.
    ///
    /// The host shows these in a docked numeric row, so a value can be typed or nudged
    /// instead of dragged. Empty (the default) means the handle has no honest number to
    /// type — the host then draws only the handle.
    fn numeric_fields(&self) -> Vec<NumericField> {
        Vec::new()
    }

    /// Adopt typed values, in the order [`numeric_fields`](Self::numeric_fields) and the
    /// commit use.
    ///
    /// The manipulator clamps and snaps them with its own rules, marks the value pending
    /// and rebuilds its preview. Returns whether the value changed.
    fn set_values(&mut self, _values: &[f64]) -> bool {
        false
    }

    /// The commit the current value would produce, without ending a drag.
    ///
    /// A typed value has no gesture to release, so the host needs the same neutral data
    /// [`drag_end`](Self::drag_end) returns. `None` when the current value is not an edit
    /// (a stem at zero) or the manipulator never commits.
    fn pending_commit(&self) -> Option<InteractionCommit> {
        None
    }

    /// Adopt a **hard** value range measured outside the manipulator (the kernel's).
    ///
    /// [`numeric_fields`](Self::numeric_fields) publishes the range from the geometry the
    /// stage itself carries; this is replaced by the document half's answer when one
    /// exists, so the row shows the value that actually fails rather than one that merely
    /// could not fit. The manipulator clamps its value and reports the change so the host
    /// can refresh a held commit. `false` (the default) means "this handle has no range to
    /// take".
    fn set_value_bounds(&mut self, _min: Option<f64>, _max: Option<f64>) -> bool {
        false
    }

    /// Adopt the value an existing feature already holds (the document half's answer).
    ///
    /// The probe reports what the selection carries — the signed length of the
    /// extrusion that produced the picked face, the radius of the fillet whose round it
    /// is — so the handle opens where the feature is and the same gesture *edits* it.
    /// This is **not** a pending edit: the value is where the handle rests, and only a
    /// drag or a typed number makes a commit. A handle the user is already editing (a
    /// drag, a typed value, a held commit) refuses the seed. `false` (the default) means
    /// "this handle has no such notion".
    fn seed_values(&mut self, _values: &[f64]) -> bool {
        false
    }

    /// Attach a short note the numeric row prints after the value.
    ///
    /// The document half's probe uses this to say what the gesture means — `editing the
    /// existing extrusion` — which is the visible difference between changing a feature
    /// and adding one. `false` (the default) means the handle has no note slot.
    fn set_note(&mut self, _note: Option<String>) -> bool {
        false
    }

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

    /// The device point the gesture radiates from: an arc's centre, a stem's base.
    ///
    /// A probe has to pull *outward* — the manipulators' own maths measures the distance
    /// from here, not the pointer's travel — so the point is part of the contract the
    /// same way the commit is. The default is `None`: a handle whose value is not a
    /// radius or an axis has no such point, and a caller falls back to its own direction.
    fn anchor_device(&self, _camera: &Camera) -> Option<[f64; 2]> {
        None
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
