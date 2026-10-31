//! Drawing producers and the screen-space math shared by the stage's overlays.
//!
//! Each producer is pure arithmetic: it returns vertices for a viewport queue and
//! knows nothing about gpui, a kernel or Python. [`grid`] and [`axis`] draw the world
//! reference, [`feedback`] draws snap feedback, and [`screen`] is the
//! projection/pixel-size helper the gizmo and workbench overlays share.

pub mod axis;
pub mod feedback;
pub mod grid;
pub mod screen;

pub use axis::AxisStyle;
pub use grid::{GridGeometry, GridSpec, GridStyle, Ground, MinorStyle};
pub use screen::{HIT_PADDING_PIXELS, HIT_SLOP, HIT_TOLERANCE_PIXELS, STROKE_PIXELS, scale_for};
