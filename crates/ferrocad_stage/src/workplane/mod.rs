//! Planar frames: the workplane state and the `+N` offset stem.
//!
//! `n = u × v` is right-handed; the world frame is the origin with `+X`/`+Y`/`+Z`. A
//! datum or a planar face produces another frame with the same operations. [`stem`]
//! is the viewport handle that offsets the active plane along its own normal.

pub mod stem;
pub mod workplane;

pub use stem::{
    STEM_PIXELS, WORKPLANE_ENTITY, WORKPLANE_OFFSET_ACTION, WORKPLANE_OFFSET_ID,
    WORKPLANE_STEM_PICK_ID, WorkplaneLive, WorkplaneStem,
};
pub use workplane::{FaceFrame, StandardPlane, Workplane, WorkplaneKind};
