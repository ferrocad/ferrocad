//! The producer seam: the producer-neutral visual state and the [`NavicubeView`] trait.
//!
//! Highlight and away-face appearance are not draw-contract features — the contract
//! carries only `color` and `world_transform` per packet — so a producer realises them
//! by recolouring pieces (and drawing away faces first), as the model's colour rule
//! describes. See `docs/navicube-spec.md`.

use ferrocad_graphics::{RenderQueue, Viewport};
use ferrocad_types::Rotation;

use super::model::{NaviTarget, Piece};

/// What to draw this frame — the cube's interaction state, independent of styling.
pub struct NavicubeVisual {
    /// The main camera's view rotation, mirrored onto the cube.
    pub orientation: Rotation,
    /// The hovered piece / Home button, if any (clamped to a `Highlighted` role).
    pub hilite: Option<NaviTarget>,
    /// The away-facing faces (recoloured, drawn first).
    pub away: Vec<Piece>,
}

/// A producer's rendering of the navigation cube (a legacy `Scene`, or modern items).
pub trait NavicubeView {
    /// Rebuild the overlay packets for `visual`, compiled at `viewport` (its
    /// device-pixel ratio seeds any screen-space geometry). Called only when the visual
    /// state changed.
    fn rebuild(&mut self, viewport: &Viewport, visual: &NavicubeVisual);

    /// The current overlay draw packets.
    fn queue(&self) -> &RenderQueue;

    /// Whether this producer can draw face/Home **labels**. A producer without a
    /// texture path returns `false` and draws untextured pieces.
    fn has_labels(&self) -> bool {
        false
    }
}
