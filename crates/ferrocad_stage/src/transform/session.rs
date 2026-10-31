//! The uncommitted state of one transform gesture.
//!
//! A transform **session** is the pending value a gesture accumulates: the datum it
//! started from, the delta measured against that datum, and the mode that opened it. It
//! exists because the preview used to be three partial copies of the same fact — a matrix
//! written onto the display item, an overlay snapshot that restored it, and the dialog's
//! mirrored field — and the three drifted apart. See
//! `docs/project/tasks/transform-session.md`.
//!
//! The session is deliberately a plain value: no `Document`, no
//! [`ShapeItem`](crate::shape::ShapeItem), no renderer. The shell owns it, the viewport
//! renders it, and the commit turns it into the neutral
//! [`InteractionCommit`](crate::InteractionCommit) the mod layer already accepts.
//! Living here rather than in gpui is what lets the toy and parametric editions and the
//! headless tests share one implementation.
//!
//! The datum is a **world point**, not a [`Placement`](ferrocad_types::Placement).
//! Document items arrive on the stage as world-space tessellations with an identity
//! placement, so an item's placement says nothing about where its origin is;
//! [`anchor`](crate::shape::ShapeItem::anchor) does. It is also the point the triad
//! already hugs, so the ghost ring, the measuring tape and the gizmo all agree about
//! where "here" is.
//!
//! A session moves a **set** of stage entities, not one. A selection of several objects is
//! one gesture: it accumulates one delta and, at commit, becomes one transaction. Keeping
//! the set in the session is what lets the commit route stop saying "move the selection"
//! item by item and start saying one commit per session.
//!
//! ```
//! use ferrocad_stage::{ShapeItem, Stage, TransformSession};
//!
//! let item = ShapeItem::new(7).with_origin(Some([1.0, 2.0, 0.0]));
//! let stage = Stage::new().with_item(item);
//!
//! let mut session = TransformSession::begin(&stage, &[7]).expect("the item is on the stage");
//! session.translate_axis(0, 5.0);
//! assert!(session.is_dirty());
//! assert_eq!(session.delta().x, 5.0);
//! ```

use ferrocad_types::{Matrix4, Placement, Rotation, Vector3};

use crate::gizmo::GizmoMode;
use crate::stage::Stage;

/// One cumulative transform gesture against one or more stage entities.
///
/// Consecutive handles, typed overrides and checkpoints all feed the same session, so a
/// gesture that is adjusted four times is still one pending value and one commit. The same
/// holds across entities: a multi-object move is one session, so the commit route can
/// produce one undo transaction for the whole selection instead of one per item. The delta
/// is held as a vector rather than as the matrix it becomes, so
/// [`translate_axis`](Self::translate_axis) can overwrite one component without
/// decomposing anything.
///
/// Rotation shares the session as **one** `(axis, angle)` step, because `SO(3)` is not a
/// vector space: composing independent per-axis angles would make the same three numbers a
/// different pose depending on their order. [`transform`](Self::transform) turns the step
/// into the matrix the stage draws. A rotation has no translation, so its delta stays zero
/// and the measuring tape draws nothing.
#[derive(Clone, Debug)]
pub struct TransformSession {
    /// The stage entities the gesture moves, in selection order with the unknown ones
    /// dropped. The session names no document object — the commit crosses that boundary
    /// through the facade, which is what keeps this crate free of the document.
    targets: Vec<u32>,
    /// The entity that owns [`datum`](Self::datum): the first target that reports an
    /// anchor. Kept beside the list rather than recomputed so [`targets`](Self::targets)
    /// can stay a plain slice and the datum and the primary can never disagree.
    primary: u32,
    /// The world-space origin captured at [`begin`](Self::begin). The ghost ring and the
    /// measuring tape sit here, and a rollback is measured from here.
    datum: Vector3,
    /// The accumulated translation from the datum, in world units (millimetres in this
    /// stack). Absolute, never per-tick: see [`set_translation`](Self::set_translation).
    delta: Vector3,
    /// The gesture kind that opened the session. `Translate` reads
    /// [`delta`](Self::delta); `Rotate` reads the pending step.
    mode: GizmoMode,
    /// The unit world axis of the one pending rotation step, `None` before a ring or a
    /// typed angle names one.
    ///
    /// A session is **single-axis** by construction: `SO(3)` is not a vector space, so
    /// three independent per-axis angles would be a lie that decomposes into fractional
    /// Euler values. Grabbing another ring commits this step first (the shell does that);
    /// the value the user sees is always `(axis, angle)`.
    axis: Option<Vector3>,
    /// The signed angle about [`axis`](Self::axis), in degrees, bounded to `(-180, 180]`.
    degrees: f64,
    /// The ring the pending step was made on (`0` = X, `1` = Y, `2` = Z), for the
    /// docked bar's axis selector. `None` for a step that cancels an arbitrary orientation
    /// (`Reset 0°`), whose axis need not be a coordinate axis.
    axis_index: Option<usize>,
    /// The body's absolute orientation when the session opened, so a local ring frame can
    /// be derived from it. Identity when the host has no document orientation (the stage
    /// is world-space, so this is the only way to know it).
    orientation_0: Rotation,
}

impl TransformSession {
    /// Open a session on `targets`, pinning the primary's anchor as the datum.
    ///
    /// Ids are kept in the order the caller supplied them, and ids the stage has no item
    /// for are dropped: a pick can name an entity the stage has not got yet, and a session
    /// must not carry a target it cannot draw. An empty slice, or one whose entities are
    /// all absent, has no session, because there would be nothing to move.
    ///
    /// The datum is the anchor of the first surviving target that reports one. That target
    /// is the [`primary`](Self::primary): the selection is not a body, so there is no
    /// single origin to use, and an average or a bounding-box centre would be a point the
    /// user did not pick and cannot see. Anchoring on one real object's origin means the
    /// triad, the ghost ring and the measuring tape hug something the user recognises, and
    /// a single-object move behaves exactly as it always did.
    ///
    /// The anchor comes from [`anchor`](crate::shape::ShapeItem::anchor), not from the
    /// item's placement: document items are world-space tessellations with an identity
    /// placement, so the anchor is the world origin while the placement is always the
    /// identity.
    ///
    /// `None` when no surviving target reports an anchor (no declared origin and no
    /// geometry). A session with no datum would have to invent the world origin as one, and
    /// every readout and rollback would then be measured from a place the objects are not.
    ///
    /// The delta starts at zero, so a session opened and committed without a gesture
    /// produces no transaction.
    pub fn begin(stage: &Stage, targets: &[u32]) -> Option<Self> {
        let mut surviving = Vec::with_capacity(targets.len());
        let mut primary = None;
        for &target in targets {
            let Some(item) = stage.item(target) else {
                continue;
            };
            surviving.push(target);
            if primary.is_none() {
                primary = item.anchor().map(|anchor| (target, anchor));
            }
        }
        let (primary, datum) = primary?;
        Some(Self {
            targets: surviving,
            primary,
            datum,
            delta: Vector3::zero(),
            mode: GizmoMode::default(),
            axis: None,
            degrees: 0.0,
            axis_index: None,
            orientation_0: Rotation::identity(),
        })
    }

    /// Set the gesture kind.
    ///
    /// The mode is recorded now, not acted on: a `Rotate` session still only translates
    /// until a ring drag feeds it. It survives [`checkpoint`](Self::checkpoint) because
    /// Enter starts the next step of the *same* verb, and the host keeps the same readout
    /// armed.
    pub fn set_mode(&mut self, mode: GizmoMode) {
        self.mode = mode;
    }

    /// Set the absolute translation along world axis `0` (`X`), `1` (`Y`) or `2` (`Z`).
    ///
    /// The value is absolute, not incremental: `translate_axis(0, 15.0)` means "15 from the
    /// datum along X" whether it follows a drag or a typed field, so typing `20` after a
    /// drag to `15` replaces the drag instead of adding to it. An index past `2` is ignored
    /// rather than clamped — it is a caller bug, and quietly moving the item along `Z`
    /// would hide the bug where it is hardest to find.
    pub fn translate_axis(&mut self, axis: usize, value: f64) {
        match axis {
            0 => self.delta.x = value,
            1 => self.delta.y = value,
            2 => self.delta.z = value,
            _ => {}
        }
    }

    /// Set the whole accumulated translation at once (absolute, like
    /// [`translate_axis`](Self::translate_axis)).
    ///
    /// This is the entry point a gizmo drag uses: a solver reports one world-space delta per
    /// tick, measured against the session datum, never an increment. Repeated ticks
    /// therefore cannot drift, and a dropped tick self-corrects on the next one.
    pub fn set_translation(&mut self, delta: Vector3) {
        self.delta = delta;
    }

    /// The stage entities the session moves, in the order they were supplied, with the
    /// absent ones dropped at [`begin`](Self::begin).
    pub fn targets(&self) -> &[u32] {
        &self.targets
    }

    /// The entity whose anchor is the [`datum`](Self::datum): the first target that reports
    /// one.
    ///
    /// Not guessed: this is the target the triad and the ghost hug, and the one a
    /// single-object session has always named. Its anchor is also what a rollback returns
    /// to. The slice is never empty and always contains the primary, so this needs no
    /// `Option` — the session could not have been opened without one.
    pub fn primary(&self) -> u32 {
        self.primary
    }

    /// The world-space datum captured at [`begin`](Self::begin): the ghost's base, and the
    /// point a rollback would return to.
    pub fn datum(&self) -> Vector3 {
        self.datum
    }

    /// The accumulated translation from the datum, in world units.
    pub fn delta(&self) -> Vector3 {
        self.delta
    }

    /// The pending rotation as a quaternion, identity while clean.
    ///
    /// The session stores a **single** axis and angle; this is the quaternion the two
    /// values name, never a composition of independent axes.
    pub fn rotation(&self) -> Rotation {
        match self.axis {
            Some(axis) if self.degrees != 0.0 => {
                Rotation::from_axis_angle(&axis, self.degrees.to_radians())
            }
            _ => Rotation::identity(),
        }
    }

    /// Set the single pending step around a named world axis.
    ///
    /// `axis_index` names the ring (`0` = X, `1` = Y, `2` = Z) for the docked axis
    /// selector; `axis` is the **world** direction the ring turns about, which is the
    /// body's local axis when a local ring frame is in use. A degenerate axis is ignored.
    /// The angle is stored signed, bounded to `(-180, 180]`.
    pub fn set_rotation_step(&mut self, axis_index: usize, axis: Vector3, degrees: f64) {
        if axis.sqr() <= f64::EPSILON {
            return;
        }
        self.axis = Some(axis.normalize());
        self.degrees = wrap_signed_degrees(degrees);
        self.axis_index = Some(axis_index.min(2));
    }

    /// Set the pending step around an arbitrary world axis (no ring label).
    ///
    /// This is what `Reset 0°` uses: cancelling a body's committed orientation is a
    /// rotation about whichever axis that orientation names, not necessarily X, Y or Z.
    pub fn set_rotation_axis_angle(&mut self, axis: Vector3, degrees: f64) {
        if axis.sqr() <= f64::EPSILON {
            return;
        }
        self.axis = Some(axis.normalize());
        self.degrees = wrap_signed_degrees(degrees);
        self.axis_index = None;
    }

    /// Change the step's signed angle without touching its axis.
    pub fn set_pending_degrees(&mut self, degrees: f64) {
        self.degrees = wrap_signed_degrees(degrees);
    }

    /// The unit world axis of the pending step, if one is named.
    pub fn pending_axis(&self) -> Option<Vector3> {
        self.axis
    }

    /// The ring the pending step was made on (`0..=2`), if it was a coordinate ring.
    pub fn pending_axis_index(&self) -> Option<usize> {
        self.axis_index
    }

    /// The signed pending angle in degrees, `0.0` while clean.
    pub fn pending_degrees(&self) -> f64 {
        self.degrees
    }

    /// Pin the body's absolute orientation at the session's start.
    ///
    /// The stage items are world-space meshes, so the document's `Placement.Rotation` is
    /// not available here; the host supplies it. A local ring frame (`LOC`) turns the
    /// selected coordinate axis by this value, and `Reset 0°` commits its inverse.
    pub fn set_orientation_0(&mut self, orientation: Rotation) {
        self.orientation_0 = orientation;
    }

    /// The absolute orientation at the session's start (identity when unknown).
    pub fn orientation_0(&self) -> Rotation {
        self.orientation_0
    }

    /// The body's absolute orientation with the pending step applied.
    pub fn absolute_rotation(&self) -> Rotation {
        self.rotation().multiply(&self.orientation_0)
    }

    /// The gesture kind the session is running as. The host reads it to pick the readout
    /// and tape form (`ΔX … mm` or `ΔθZ … °`).
    pub fn mode(&self) -> GizmoMode {
        self.mode
    }

    /// The matrix every target item is drawn with during the session.
    ///
    /// A translation is the accumulated delta; a rotation is
    /// `T(datum)·R(axis, θ)·T(−datum)`, so the body turns about the pivot the triad hugs
    /// instead of orbiting the world origin. The caller composes the matrix *after* the
    /// item's placement (the `temporary` slot of
    /// [`set_temporary_transform`](crate::shape::ShapeItem::set_temporary_transform)) and
    /// leaves the geometry itself untouched until the commit. It is the same matrix for the
    /// whole selection because a gesture moves the selection rigidly; the datum is not part
    /// of a translation — the geometry already sits where it is in world space, so adding
    /// the datum would move the items twice.
    pub fn transform(&self) -> Matrix4 {
        match self.mode {
            // The rotation is about the session **datum**, not the world origin: the
            // matrix is composed after the item's placement, so a body away from the
            // world origin must spin about the pivot the triad hugs instead of orbiting.
            GizmoMode::Rotate if self.axis.is_some() && self.degrees != 0.0 => {
                let rotation = Placement::new(Vector3::zero(), self.rotation()).to_matrix();
                Matrix4::translation(self.datum.x, self.datum.y, self.datum.z)
                    .mul(&rotation)
                    .mul(&Matrix4::translation(
                        -self.datum.x,
                        -self.datum.y,
                        -self.datum.z,
                    ))
            }
            _ => Matrix4::translation(self.delta.x, self.delta.y, self.delta.z),
        }
    }

    /// Enter: commit one step and re-arm at `datum`.
    ///
    /// The caller supplies the new datum rather than the session deriving it from the old
    /// datum plus the delta, so a rotate step can pass a point that a translation could not
    /// have produced. Zeroing the delta is what makes the next gesture start from the pose
    /// just committed instead of adding to a delta that has already been applied once.
    pub fn checkpoint(&mut self, datum: Vector3) {
        self.datum = datum;
        self.delta = Vector3::zero();
        // The committed pose becomes the next step's starting orientation.
        self.orientation_0 = self.absolute_rotation();
        self.axis = None;
        self.degrees = 0.0;
        self.axis_index = None;
    }

    /// Whether the session holds a translation worth committing.
    ///
    /// The comparison is exact because both writers store absolute values: a delta is
    /// either exactly zero (nothing typed, nothing dragged) or a value the user chose. A
    /// drag that returns to its datum is therefore clean again — no transaction, no undo
    /// entry — rather than leaving a near-zero that merely looks clean.
    pub fn is_dirty(&self) -> bool {
        self.delta.x != 0.0
            || self.delta.y != 0.0
            || self.delta.z != 0.0
            || (self.axis.is_some() && self.degrees != 0.0)
    }
}

/// Normalize a signed angle to `(-180, 180]` so a drag that crosses half a turn keeps a
/// readable value (a `+190°` turn is the same pose as `-170°`). Zero never becomes `-0.0`.
fn wrap_signed_degrees(degrees: f64) -> f64 {
    let mut wrapped = degrees % 360.0;
    if wrapped <= -180.0 {
        wrapped += 360.0;
    } else if wrapped > 180.0 {
        wrapped -= 360.0;
    }
    if wrapped == 0.0 { 0.0 } else { wrapped }
}

/// The measuring tape's model: where a session started and how far it has moved.
///
/// The viewport turns this into screen-invariant geometry each paint (a ghost ring at the
/// datum and dashed stair-step legs to the previewed origin). It is a **copy**, not a
/// borrow: the tape outlives the shell update that produced it and a later tick must not
/// mutate geometry that is already queued. The shell builds it from the live session; the
/// viewport owns the drawing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransformTape {
    datum: Vector3,
    delta: Vector3,
    /// The axes the legs follow: the active transform frame (`WLD` / `LOC` / `WP`).
    /// [`new`](Self::new) uses the coordinate axes, which is the historical world tape.
    basis: [Vector3; 3],
}

impl TransformTape {
    /// A world-axis tape.
    pub fn new(datum: Vector3, delta: Vector3) -> Self {
        Self {
            datum,
            delta,
            basis: world_axes(),
        }
    }

    /// A tape whose legs follow `basis` (the active transform frame).
    pub fn in_frame(datum: Vector3, delta: Vector3, basis: [Vector3; 3]) -> Self {
        Self {
            datum,
            delta,
            basis,
        }
    }

    /// Snapshot a live session: its datum is the tape's anchor and its absolute delta is
    /// the offset.
    pub fn from_session(session: &TransformSession) -> Self {
        Self::new(session.datum(), session.delta())
    }

    /// Snapshot a live session in the transform frame the form is using, so the ruler
    /// measures the same axes the handles and the fields do.
    pub fn from_session_in_frame(session: &TransformSession, basis: [Vector3; 3]) -> Self {
        Self::in_frame(session.datum(), session.delta(), basis)
    }

    /// The world point the tape starts at (the session datum).
    pub fn datum(&self) -> Vector3 {
        self.datum
    }

    /// The absolute offset from the datum.
    pub fn delta(&self) -> Vector3 {
        self.delta
    }

    /// The signed displacement along frame axis `axis` (`0` = X, `1` = Y, `2` = Z).
    pub fn axis_delta(&self, axis: usize) -> f64 {
        self.delta.dot(&self.basis[axis.min(2)])
    }

    /// The world direction of frame axis `axis`.
    pub fn axis(&self, axis: usize) -> Vector3 {
        self.basis[axis.min(2)]
    }

    /// The frame the tape measures in.
    pub fn basis(&self) -> [Vector3; 3] {
        self.basis
    }

    /// The previewed origin the tape ends at.
    pub fn current(&self) -> Vector3 {
        self.datum.add(&self.delta)
    }

    /// Whether there is an offset worth drawing. A clean session has no tape.
    pub fn is_visible(&self) -> bool {
        self.delta.x != 0.0 || self.delta.y != 0.0 || self.delta.z != 0.0
    }
}

/// The coordinate axes as a frame.
fn world_axes() -> [Vector3; 3] {
    [
        Vector3::new(1.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::ShapeItem;

    /// A triangle with a declared origin, the way a display provider supplies a document
    /// item: world-space geometry plus the object's own placement point.
    fn block(entity: u32, origin: [f32; 3]) -> ShapeItem {
        ShapeItem::new(entity)
            .with_origin(Some(origin))
            .with_shape(
                vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                vec![],
                vec![0, 1, 2],
                vec![],
            )
    }

    #[test]
    fn begin_pins_the_items_anchor_as_the_datum() {
        let stage = Stage::new().with_item(block(4, [10.0, -2.0, 5.0]));
        let session = TransformSession::begin(&stage, &[4]).expect("the entity is on the stage");

        assert_eq!(session.targets(), &[4]);
        assert_eq!(session.primary(), 4);
        assert_eq!(session.datum(), Vector3::new(10.0, -2.0, 5.0));
        assert_eq!(session.delta(), Vector3::zero());
        assert!(!session.is_dirty());
    }

    /// A multi-object move is one session, and the order the targets were picked in is the
    /// order they are moved in. Unknown ids are dropped rather than carried: the session
    /// cannot draw an entity the stage has not got.
    #[test]
    fn begin_keeps_the_targets_and_drops_the_unknown_ones() {
        let stage = Stage::new()
            .with_item(block(4, [0.0; 3]))
            .with_item(block(7, [0.0; 3]));

        let session = TransformSession::begin(&stage, &[7, 99, 4]).expect("two targets survive");

        assert_eq!(session.targets(), &[7, 4]);
        assert_eq!(session.primary(), 7, "the first surviving target owns the datum");
    }

    /// The datum is one real object's anchor, not a selection-wide average: the first target
    /// that reports an anchor wins, even if an earlier target has none.
    #[test]
    fn the_datum_comes_from_the_first_target_with_an_anchor() {
        let stage = Stage::new()
            .with_item(ShapeItem::new(9))
            .with_item(block(4, [3.0, 6.0, 9.0]))
            .with_item(block(3, [-1.0, -1.0, -1.0]));

        let session = TransformSession::begin(&stage, &[9, 4, 3]).expect("4 supplies the datum");

        assert_eq!(
            session.targets(),
            &[9, 4, 3],
            "a known item without an anchor is still moved, it just cannot own the datum"
        );
        assert_eq!(session.primary(), 4);
        assert_eq!(session.datum(), Vector3::new(3.0, 6.0, 9.0));
    }

    /// No declared origin: the anchor falls back to the bounding box's base corner, and the
    /// datum follows it rather than the identity placement.
    #[test]
    fn begin_falls_back_to_the_bounding_box_corner() {
        let item = ShapeItem::new(3).with_shape(
            vec![[2.0, 3.0, 4.0], [5.0, 3.0, 9.0]],
            vec![],
            vec![],
            vec![],
        );
        let stage = Stage::new().with_item(item);

        let session = TransformSession::begin(&stage, &[3]).expect("the entity is on the stage");
        assert_eq!(session.datum(), Vector3::new(2.0, 3.0, 4.0));
    }

    #[test]
    fn an_empty_selection_has_no_session() {
        let stage = Stage::new().with_item(block(4, [0.0; 3]));
        assert!(TransformSession::begin(&stage, &[]).is_none());
    }

    #[test]
    fn an_unknown_entity_has_no_session() {
        let stage = Stage::new().with_item(block(4, [0.0; 3]));
        assert!(TransformSession::begin(&stage, &[99]).is_none());
    }

    #[test]
    fn only_unknown_entities_have_no_session() {
        let stage = Stage::new().with_item(block(4, [0.0; 3]));
        assert!(TransformSession::begin(&stage, &[98, 99]).is_none());
    }

    #[test]
    fn an_item_without_an_anchor_has_no_session() {
        let stage = Stage::new().with_item(ShapeItem::new(5));
        assert!(TransformSession::begin(&stage, &[5]).is_none());
    }

    #[test]
    fn the_delta_is_absolute_not_incremental() {
        let stage = Stage::new().with_item(block(1, [0.0; 3]));
        let mut session = TransformSession::begin(&stage, &[1]).unwrap();

        session.translate_axis(0, 15.0);
        session.translate_axis(0, 20.0);
        assert_eq!(
            session.delta(),
            Vector3::new(20.0, 0.0, 0.0),
            "a typed value replaces the axis, it does not add to it"
        );

        session.translate_axis(1, -3.5);
        session.set_translation(Vector3::new(1.0, 2.0, 3.0));
        assert_eq!(
            session.delta(),
            Vector3::new(1.0, 2.0, 3.0),
            "set_translation replaces every axis"
        );
        assert!(session.is_dirty());

        session.translate_axis(7, 100.0);
        assert_eq!(
            session.delta(),
            Vector3::new(1.0, 2.0, 3.0),
            "an out-of-range axis is ignored, not clamped"
        );
    }

    #[test]
    fn the_transform_is_the_accumulated_translation() {
        let stage = Stage::new().with_item(block(1, [0.0; 3]));
        let mut session = TransformSession::begin(&stage, &[1]).unwrap();
        assert_eq!(
            session.transform(),
            Matrix4::identity(),
            "a fresh session draws the item where it already is"
        );

        session.translate_axis(0, 1.5);
        session.translate_axis(1, 2.0);
        assert_eq!(session.transform(), Matrix4::translation(1.5, 2.0, 0.0));
        assert_eq!(
            session.transform().transform(&Vector3::new(1.0, 2.0, 3.0)),
            Vector3::new(2.5, 4.0, 3.0),
            "the datum is not part of the matrix: the item is already there"
        );
    }

    #[test]
    fn a_checkpoint_moves_the_datum_and_clears_the_delta() {
        let stage = Stage::new().with_item(block(1, [0.0; 3]));
        let mut session = TransformSession::begin(&stage, &[1]).unwrap();
        session.set_translation(Vector3::new(0.0, 0.0, 7.0));

        session.checkpoint(Vector3::new(10.0, 2.0, 12.0));

        assert_eq!(session.datum(), Vector3::new(10.0, 2.0, 12.0));
        assert_eq!(session.delta(), Vector3::zero());
        assert!(!session.is_dirty());
        assert_eq!(session.transform(), Matrix4::identity());
    }

    #[test]
    fn a_zero_delta_is_clean_again() {
        let stage = Stage::new().with_item(block(1, [0.0; 3]));
        let mut session = TransformSession::begin(&stage, &[1]).unwrap();

        session.set_translation(Vector3::new(4.0, 0.0, 0.0));
        assert!(session.is_dirty());

        // A drag that ends where it started leaves nothing to commit.
        session.set_translation(Vector3::zero());
        assert!(!session.is_dirty());
    }

    #[test]
    fn the_mode_is_recorded_without_changing_the_translation() {
        let stage = Stage::new().with_item(block(1, [0.0; 3]));
        let mut session = TransformSession::begin(&stage, &[1]).unwrap();
        assert_eq!(
            session.mode(),
            GizmoMode::Translate,
            "a session starts on the default kind"
        );

        session.set_translation(Vector3::new(1.0, 0.0, 0.0));
        session.set_mode(GizmoMode::Rotate);

        assert_eq!(session.mode(), GizmoMode::Rotate);
        assert_eq!(
            session.transform(),
            Matrix4::translation(1.0, 0.0, 0.0),
            "rotation joins the matrix when a ring gesture exists, not before"
        );
    }

    #[test]
    fn a_tape_snapshots_the_session_datum_and_delta() {
        let stage = Stage::new().with_item(block(1, [2.0, 3.0, 0.0]));
        let mut session = TransformSession::begin(&stage, &[1]).unwrap();
        session.set_translation(Vector3::new(5.0, 0.0, 0.0));

        let tape = TransformTape::from_session(&session);

        assert_eq!(tape.datum(), Vector3::new(2.0, 3.0, 0.0));
        assert_eq!(tape.delta(), Vector3::new(5.0, 0.0, 0.0));
        assert_eq!(
            tape.current(),
            Vector3::new(7.0, 3.0, 0.0),
            "the tape ends at the previewed origin"
        );
    }

    /// A rotation session is **one** pending `(axis, angle)` step, not three independent
    /// Euler fields: setting another step replaces the first instead of composing axes.
    #[test]
    fn a_rotation_session_holds_one_axis_and_one_angle() {
        let stage = Stage::new().with_item(block(7, [5.0, 0.0, 0.0]));
        let mut session =
            TransformSession::begin(&stage, &[7]).expect("the entity is on the stage");
        session.set_mode(GizmoMode::Rotate);
        assert!(
            !session.is_dirty(),
            "a rotate session with no sweep is clean"
        );

        // The axis is normalised, so a scaled frame axis is still one value.
        session.set_rotation_step(2, Vector3::new(0.0, 0.0, 2.0), 90.0);
        assert!(session.is_dirty());
        assert_eq!(session.pending_axis_index(), Some(2));
        assert!((session.pending_degrees() - 90.0).abs() < 1e-9);
        // The datum (5, 0, 0) is the pivot: it stays fixed while +X from it turns to +Y.
        let pivot = session.transform().transform(&Vector3::new(5.0, 0.0, 0.0));
        assert!(
            pivot.sub(&Vector3::new(5.0, 0.0, 0.0)).length() < 1e-9,
            "the pivot does not move: {pivot:?}"
        );
        let turned = session.transform().transform(&Vector3::new(6.0, 0.0, 0.0));
        assert!(
            turned.sub(&Vector3::new(5.0, 1.0, 0.0)).length() < 1e-9,
            "+90 deg about the datum maps +X of it to +Y: {turned:?}"
        );

        // A second step on another axis **replaces** the first: the pending value is one
        // axis, so it can never decompose into fractional cross-talk.
        session.set_rotation_step(0, Vector3::new(1.0, 0.0, 0.0), 30.0);
        assert_eq!(session.pending_axis_index(), Some(0));
        assert!((session.pending_degrees() - 30.0).abs() < 1e-9);
        let pivot = session.transform().transform(&Vector3::new(5.0, 0.0, 0.0));
        let turned = session
            .transform()
            .transform(&Vector3::new(5.0, 0.0, 1.0))
            .sub(&pivot);
        // +30 about X maps the pivot's +Z onto (0, -sin 30, cos 30).
        assert!(
            turned
                .sub(&Vector3::new(
                    0.0,
                    -30.0f64.to_radians().sin(),
                    30.0f64.to_radians().cos(),
                ))
                .length()
                < 1e-9,
            "the pending pose is exactly +30 about X: {turned:?}"
        );

        // The displayed angle is bounded to (-180, 180]; the rotation is unchanged.
        session.set_rotation_step(2, Vector3::new(0.0, 0.0, 1.0), 190.0);
        assert!((session.pending_degrees() + 170.0).abs() < 1e-9);
        session.set_rotation_step(2, Vector3::new(0.0, 0.0, 1.0), -180.0);
        assert!((session.pending_degrees() - 180.0).abs() < 1e-9);

        // A checkpoint clears the step with the translation.
        let datum = session.datum();
        session.checkpoint(datum);
        assert!(!session.is_dirty());
        assert_eq!(session.pending_axis(), None);
        assert_eq!(session.pending_degrees(), 0.0);
        assert!(session.rotation().is_same(&Rotation::identity(), 1e-12));
        assert!(session.transform().is_unity(1e-9));

        // A degenerate axis has nothing to turn about.
        session.set_rotation_axis_angle(Vector3::zero(), 45.0);
        assert_eq!(session.pending_axis(), None);
        assert!(!session.is_dirty());
    }

    /// A 15° detent is **exactly** 15° on the one axis: the value the bar shows and the
    /// rotation the matrix applies are the same scalar, with no Euler decomposition.
    #[test]
    fn a_snapped_ring_step_is_exactly_the_detent() {
        let stage = Stage::new().with_item(block(7, [0.0; 3]));
        let mut session =
            TransformSession::begin(&stage, &[7]).expect("the entity is on the stage");
        session.set_mode(GizmoMode::Rotate);
        for (index, axis) in [
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        ]
        .into_iter()
        .enumerate()
        {
            session.set_rotation_step(index, axis, 15.0);
            assert!((session.pending_degrees() - 15.0).abs() < 1e-12);
            let angle = session.rotation().angle().to_degrees();
            assert!(
                (angle - 15.0).abs() < 1e-9,
                "the quaternion is the same 15 deg step: {angle}"
            );
        }
    }

    /// A step that cancels an arbitrary orientation has no coordinate-ring label; the
    /// `Reset 0°` action uses it.
    #[test]
    fn a_reset_step_can_have_no_ring_label() {
        let stage = Stage::new().with_item(block(7, [0.0; 3]));
        let mut session =
            TransformSession::begin(&stage, &[7]).expect("the entity is on the stage");
        session.set_mode(GizmoMode::Rotate);
        let orientation = Rotation::from_euler_xyz_deg(10.0, 20.0, 30.0);
        session.set_orientation_0(orientation);
        let inverse = session.orientation_0().inverse();
        let mut degrees = inverse.angle().to_degrees();
        let mut axis = inverse.axis();
        if degrees > 180.0 {
            degrees -= 360.0;
            axis = axis.scale(-1.0);
        }
        session.set_rotation_axis_angle(axis, degrees);
        assert_eq!(session.pending_axis_index(), None);
        assert!(
            session.rotation().is_same(&orientation.inverse(), 1e-9),
            "the step cancels the committed orientation"
        );
    }

    #[test]
    fn a_tape_measures_in_the_frame_it_is_given() {
        let half = std::f64::consts::FRAC_1_SQRT_2;
        let basis = [
            Vector3::new(half, half, 0.0),
            Vector3::new(-half, half, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
        ];
        // Ten along the rotated frame's X, which is (half, half, 0) in world space.
        let delta = Vector3::new(10.0 * half, 10.0 * half, 0.0);
        let tape = TransformTape::in_frame(Vector3::zero(), delta, basis);
        assert!((tape.axis_delta(0) - 10.0).abs() < 1e-9);
        assert!(tape.axis_delta(1).abs() < 1e-9);
        assert!(tape.axis(0).is_equal(&basis[0], 1e-9));
    }

    #[test]
    fn a_tape_is_visible_only_with_a_nonzero_delta() {
        let clean = TransformTape::new(Vector3::new(1.0, 2.0, 3.0), Vector3::zero());
        assert!(!clean.is_visible());
        for delta in [
            Vector3::new(1.0, 0.0, 0.0),
            Vector3::new(0.0, -1.0, 0.0),
            Vector3::new(0.0, 0.0, 0.5),
        ] {
            assert!(TransformTape::new(Vector3::zero(), delta).is_visible());
        }
    }
}
