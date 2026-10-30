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
//! ```
//! use ferrocad_stage::{ShapeItem, Stage, TransformSession};
//!
//! let item = ShapeItem::new(7).with_origin(Some([1.0, 2.0, 0.0]));
//! let stage = Stage::new().with_item(item);
//!
//! let mut session = TransformSession::begin(&stage, 7).expect("the item is on the stage");
//! session.translate_axis(0, 5.0);
//! assert!(session.is_dirty());
//! assert_eq!(session.delta().x, 5.0);
//! ```

use ferrocad_types::{Matrix4, Vector3};

use crate::gizmo::GizmoMode;
use crate::stage::Stage;

/// One cumulative transform gesture against one stage entity.
///
/// Consecutive handles, typed overrides and checkpoints all feed the same session, so a
/// gesture that is adjusted four times is still one pending value and one commit. The
/// delta is held as a vector rather than as the matrix it becomes, so
/// [`translate_axis`](Self::translate_axis) can overwrite one component without
/// decomposing anything.
///
/// Rotation will join [`transform`](Self::transform) once a rotate gesture exists; it is
/// deliberately absent now, because nothing emits one yet. See [`transform`](Self::transform).
#[derive(Clone, Copy, Debug)]
pub struct TransformSession {
    /// The stage entity the gesture moves. The session names no document object — the
    /// commit crosses that boundary through the facade, which is what keeps this crate
    /// free of the document.
    target: u32,
    /// The world-space origin captured at [`begin`](Self::begin). The ghost ring and the
    /// measuring tape sit here, and a rollback returns the item here.
    datum: Vector3,
    /// The accumulated translation from the datum, in world units (millimetres in this
    /// stack). Absolute, never per-tick: see [`set_translation`](Self::set_translation).
    delta: Vector3,
    /// The gesture kind that opened the session. Carried from the first slice so a rotate
    /// session becomes a widening of this type rather than a second type; only
    /// `Translate` affects [`transform`](Self::transform) today.
    mode: GizmoMode,
}

impl TransformSession {
    /// Open a session on `target`, pinning its anchor as the datum.
    ///
    /// The datum comes from [`anchor`](crate::shape::ShapeItem::anchor), not from the
    /// item's placement: document items are world-space tessellations with an identity
    /// placement, so the anchor is the world origin while the placement is always the
    /// identity.
    ///
    /// `None` when the stage has no item carrying `target`, and also when that item reports
    /// no anchor (no declared origin and no geometry). A session with no datum would have to
    /// invent the world origin as one, and every readout and rollback would then be measured
    /// from a place the object is not.
    ///
    /// The delta starts at zero, so a session opened and committed without a gesture
    /// produces no transaction.
    pub fn begin(stage: &Stage, target: u32) -> Option<Self> {
        let item = stage.item(target)?;
        Some(Self {
            target,
            datum: item.anchor()?,
            delta: Vector3::zero(),
            mode: GizmoMode::default(),
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

    /// The stage entity the session moves.
    pub fn target(&self) -> u32 {
        self.target
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

    /// The gesture kind the session is running as. The host reads it to pick the readout
    /// and tape form (`ΔX … mm` or `ΔθZ … °`).
    pub fn mode(&self) -> GizmoMode {
        self.mode
    }

    /// The matrix the target item is drawn with during the session.
    ///
    /// It is a translation only, relative to the item's committed pose: the caller composes
    /// it *after* the item's placement (the `temporary` slot of
    /// [`set_temporary_transform`](crate::shape::ShapeItem::set_temporary_transform)) and
    /// leaves the geometry itself untouched until the commit. The datum is not part of it —
    /// the geometry already sits at the datum in world space, so adding it would move the
    /// item twice.
    ///
    /// Rotation will be composed here once a gesture can produce one (`mode == Rotate`
    /// with a ring drag behind it). It is absent now on purpose: the session holds no
    /// rotation, and a matrix that silently dropped a requested rotation would be worse than
    /// one that cannot express it.
    pub fn transform(&self) -> Matrix4 {
        Matrix4::translation(self.delta.x, self.delta.y, self.delta.z)
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
    }

    /// Whether the session holds a translation worth committing.
    ///
    /// The comparison is exact because both writers store absolute values: a delta is
    /// either exactly zero (nothing typed, nothing dragged) or a value the user chose. A
    /// drag that returns to its datum is therefore clean again — no transaction, no undo
    /// entry — rather than leaving a near-zero that merely looks clean.
    pub fn is_dirty(&self) -> bool {
        self.delta.x != 0.0 || self.delta.y != 0.0 || self.delta.z != 0.0
    }
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
        let session = TransformSession::begin(&stage, 4).expect("the entity is on the stage");

        assert_eq!(session.target(), 4);
        assert_eq!(session.datum(), Vector3::new(10.0, -2.0, 5.0));
        assert_eq!(session.delta(), Vector3::zero());
        assert!(!session.is_dirty());
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

        let session = TransformSession::begin(&stage, 3).expect("the entity is on the stage");
        assert_eq!(session.datum(), Vector3::new(2.0, 3.0, 4.0));
    }

    #[test]
    fn an_unknown_entity_has_no_session() {
        let stage = Stage::new().with_item(block(4, [0.0; 3]));
        assert!(TransformSession::begin(&stage, 99).is_none());
    }

    #[test]
    fn an_item_without_an_anchor_has_no_session() {
        let stage = Stage::new().with_item(ShapeItem::new(5));
        assert!(TransformSession::begin(&stage, 5).is_none());
    }

    #[test]
    fn the_delta_is_absolute_not_incremental() {
        let stage = Stage::new().with_item(block(1, [0.0; 3]));
        let mut session = TransformSession::begin(&stage, 1).unwrap();

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
        let mut session = TransformSession::begin(&stage, 1).unwrap();
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
        let mut session = TransformSession::begin(&stage, 1).unwrap();
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
        let mut session = TransformSession::begin(&stage, 1).unwrap();

        session.set_translation(Vector3::new(4.0, 0.0, 0.0));
        assert!(session.is_dirty());

        // A drag that ends where it started leaves nothing to commit.
        session.set_translation(Vector3::zero());
        assert!(!session.is_dirty());
    }

    #[test]
    fn the_mode_is_recorded_without_changing_the_translation() {
        let stage = Stage::new().with_item(block(1, [0.0; 3]));
        let mut session = TransformSession::begin(&stage, 1).unwrap();
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
}
