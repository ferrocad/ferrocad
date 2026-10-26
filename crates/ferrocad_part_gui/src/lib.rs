//! # FerroCAD Part GUI (`ferrocad_part_gui`)
//!
//! The Part workbench's **presentation** layer: the manipulators and interaction
//! affordances that only make sense with `Part`. Today it is the edge dress-up
//! scrubber — the arc that turns a drag into a fillet radius or a chamfer distance —
//! and the face push/pull stem that turns a drag into an extrusion or a pocket.
//!
//! This crate is the answer to "where does a workbench's manipulator live". It is
//! **not** in `ferrocad_stage` (which must serve every workbench, including BIM and
//! Draft) and **not** in `ferrocad_gpui` (the edition-agnostic shell). It is a
//! presentation crate beside the workbench, and it registers through the generic
//! [`ferrocad_stage::Manipulator`] seam:
//!
//! ```text
//! ferrocad_stage        Manipulator · InteractionCommit · Stage · ShapeItem        (generic)
//!       ▲
//! ferrocad_part_gui     DressUpScrubber · PushPullStem · their factories
//!       ▲
//! the edition           registers `fillet_manipulator` and `push_pull_manipulator`
//! ```
//!
//! It names no kernel and no document: its dependencies are the generic stage
//! vocabulary, the tessellation/selection types and the camera. Applying a commit to
//! the document happens in the app's Python script — never here.
//!
//! See `docs/architecture.md` §10 and `docs/interaction/gizmo-manipulators.md`.

pub mod dress_up;
pub mod push_pull;

pub use dress_up::{
    DRESS_UP_ID, DRESS_UP_PICK_ID, DressUpChange, DressUpMode, DressUpScrubber, DressUpValue,
    MIN_RADIUS, dress_up_scrubber_for, fillet_manipulator,
};
pub use push_pull::{
    push_pull_manipulator, push_pull_stem_for, PushPullChange, PushPullStem, PUSH_PULL_ID,
    PUSH_PULL_PICK_ID,
};
