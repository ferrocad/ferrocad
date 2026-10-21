//! The producer seam: the producer-neutral visual state and the [`NavicubeView`] trait.
//!
//! Highlight and away-face appearance are not draw-contract features — the contract
//! carries only `color` and `world_transform` per packet — so a producer realises them
//! by recolouring pieces (and drawing away faces first), as the model's colour rule
//! describes. See `docs/interaction/navicube-spec.md`.

use ferrocad_graphics::{RenderQueue, Viewport};
use ferrocad_types::Rotation;

use super::model::Piece;
use super::theme::NavicubeTheme;

/// What to draw this frame — the cube's interaction state, independent of styling.
pub struct NavicubeVisual {
    /// The main camera's view rotation, mirrored onto the cube.
    pub orientation: Rotation,
    /// The hovered piece, if any (clamped to a `Highlighted` role).
    pub hilite: Option<Piece>,
    /// The away-facing faces (recoloured, drawn first).
    pub away: Vec<Piece>,
    /// Whether the pointer is anywhere in the gizmo's rect (including the negative
    /// space between pieces). Proximity brightens the wireframe and labels without
    /// hovering a specific target.
    pub proximity: bool,
    /// Whether the compass ring is hovered or being scrubbed (it brightens to cyan).
    pub ring: bool,
}

/// A producer's rendering of the navigation cube (a legacy `Scene`, or modern items).
pub trait NavicubeView {
    /// Rebuild the overlay packets for `visual`, resolving each piece's role through
    /// `theme`, compiled at `viewport` (its device-pixel ratio seeds any screen-space
    /// geometry). Called only when the visual state changed.
    fn rebuild(&mut self, viewport: &Viewport, visual: &NavicubeVisual, theme: &NavicubeTheme);

    /// The current overlay draw packets.
    fn queue(&self) -> &RenderQueue;
}
