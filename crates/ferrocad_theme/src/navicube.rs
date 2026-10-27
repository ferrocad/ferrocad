//! The navigation cube's raw palette: the built colours its role layer maps onto.
//!
//! **Convention.** Most values are **sRGB-normalised** `[f32; 4]` — each channel is
//! the `#RRGGBB` byte divided by 255, not converted to linear — so the cube's edges
//! render brighter than their nominal hex when they reach the renderer.
//! [`TURNTABLE_COLOR`] is the one documented exception: it is **linear**. The V0b
//! audit in `docs/developer/interaction/visual-language.md` §1.6 still has to normalise the
//! set onto one convention; until then these values are the build target and must
//! not change.
//!
//! This module is the palette only. The semantic `NavicubeRole` → colour mapping
//! stays with the cube (the viewport's `navicube::theme`), which re-exports these
//! names so existing paths keep working.

/// The cube's resting **glass facet** colour: a faint dark tint that gives the facet
/// volume without hiding the model behind it.
pub const BASE_COLOR: [f32; 4] = [0.18, 0.23, 0.30, 0.16];
/// The soft cyan a hovered bevel or corner takes.
pub const HILITE_COLOR: [f32; 4] = [0.28, 0.90, 1.0, 0.60];
/// A bevel adjacent to a hovered corner: a softer cyan than the hovered piece itself.
pub const NEIGHBOUR_COLOR: [f32; 4] = [0.28, 0.65, 0.95, 0.20];
/// The still fainter tint of an away-facing face.
pub const INACTIVE_COLOR: [f32; 4] = [0.14, 0.18, 0.24, 0.06];
/// A resting bevel quad: the bright machined chamfer between two face octagons.
pub const BEVEL_COLOR: [f32; 4] = [0.86, 0.92, 1.0, 0.45];
/// A bevel quad once the pointer is in the gizmo.
pub const BEVEL_PROXIMITY: [f32; 4] = [0.93, 0.96, 1.0, 0.60];
/// A bevel behind the cube: dimmed so the front wireframe dominates.
pub const BEVEL_AWAY: [f32; 4] = [0.48, 0.53, 0.60, 0.10];

/// A resting 1px outline. Front and back are deliberately far apart in alpha so the
/// wireframe cannot flip into a Necker cube; the line pipeline is 1px, so depth is
/// carried by opacity rather than stroke weight.
pub const EDGE_COLOR: [f32; 4] = [0.84, 0.88, 0.94, 0.62];
/// The outline once the pointer is in the gizmo (proximity, not a hovered piece).
pub const EDGE_PROXIMITY: [f32; 4] = [0.90, 0.94, 1.0, 0.9];
/// The outline of the hovered piece.
pub const EDGE_HILITE: [f32; 4] = [0.4, 0.84, 1.0, 1.0];
/// The outline of a bevel adjacent to a hovered corner.
pub const EDGE_NEIGHBOUR: [f32; 4] = [0.4, 0.84, 1.0, 0.80];
/// The outline of an away-facing face: a ghost line, not a second front edge.
pub const EDGE_AWAY: [f32; 4] = [0.55, 0.60, 0.68, 0.10];
/// A resting face label: bright, high-contrast ink for the filled glyphs and the compass
/// letters.
pub const LABEL_COLOR: [f32; 4] = [0.92, 0.95, 1.0, 0.80];
/// A label once the pointer is in the gizmo.
pub const LABEL_PROXIMITY: [f32; 4] = [0.98, 0.99, 1.0, 0.95];
/// The label of the hovered piece.
pub const LABEL_HILITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The soft cyan ribbon behind a hovered edge. The renderer has no bloom pass and no wide
/// lines, so "glow" is a translucent screen-space quad around the edge.
pub const GLOW_COLOR: [f32; 4] = [0.4, 0.84, 1.0, 0.22];
/// The compass ring at rest. It lies on the ground plane, so it must read as a turntable
/// rather than disappear into the viewport.
pub const RING_COLOR: [f32; 4] = [0.62, 0.68, 0.76, 0.70];
/// The ring once the pointer is in the gizmo.
pub const RING_PROXIMITY: [f32; 4] = [0.75, 0.81, 0.88, 0.90];
/// The ring while hovered or scrubbed.
pub const RING_HILITE: [f32; 4] = [0.4, 0.84, 1.0, 0.95];
/// The dark floor disc under the cube: it grounds the glass body against the viewport.
/// Values are **linear**, so this is much darker than its nominal `#141820` sRGB look.
pub const TURNTABLE_COLOR: [f32; 4] = [0.008, 0.010, 0.016, 0.72];
/// The thick rim band between the turntable's inner and outer loops.
pub const RING_FILL: [f32; 4] = [0.59, 0.725, 0.824, 0.35];
/// The rim band once the pointer is in the gizmo.
pub const RING_FILL_PROXIMITY: [f32; 4] = [0.68, 0.80, 0.89, 0.45];
/// The rim band while hovered or scrubbed.
pub const RING_FILL_HILITE: [f32; 4] = [0.4, 0.84, 1.0, 0.55];

#[cfg(test)]
mod tests {
    use super::*;

    /// The palette is a move, not a retheme: every value is the one the viewport built
    /// with before the extraction.
    #[test]
    fn the_palette_is_unchanged() {
        for (actual, expected) in [
            (BASE_COLOR, [0.18, 0.23, 0.30, 0.16]),
            (HILITE_COLOR, [0.28, 0.90, 1.0, 0.60]),
            (NEIGHBOUR_COLOR, [0.28, 0.65, 0.95, 0.20]),
            (INACTIVE_COLOR, [0.14, 0.18, 0.24, 0.06]),
            (BEVEL_COLOR, [0.86, 0.92, 1.0, 0.45]),
            (BEVEL_PROXIMITY, [0.93, 0.96, 1.0, 0.60]),
            (BEVEL_AWAY, [0.48, 0.53, 0.60, 0.10]),
            (EDGE_COLOR, [0.84, 0.88, 0.94, 0.62]),
            (EDGE_PROXIMITY, [0.90, 0.94, 1.0, 0.9]),
            (EDGE_HILITE, [0.4, 0.84, 1.0, 1.0]),
            (EDGE_NEIGHBOUR, [0.4, 0.84, 1.0, 0.80]),
            (EDGE_AWAY, [0.55, 0.60, 0.68, 0.10]),
            (LABEL_COLOR, [0.92, 0.95, 1.0, 0.80]),
            (LABEL_PROXIMITY, [0.98, 0.99, 1.0, 0.95]),
            (LABEL_HILITE, [1.0, 1.0, 1.0, 1.0]),
            (GLOW_COLOR, [0.4, 0.84, 1.0, 0.22]),
            (RING_COLOR, [0.62, 0.68, 0.76, 0.70]),
            (RING_PROXIMITY, [0.75, 0.81, 0.88, 0.90]),
            (RING_HILITE, [0.4, 0.84, 1.0, 0.95]),
            (TURNTABLE_COLOR, [0.008, 0.010, 0.016, 0.72]),
            (RING_FILL, [0.59, 0.725, 0.824, 0.35]),
            (RING_FILL_PROXIMITY, [0.68, 0.80, 0.89, 0.45]),
            (RING_FILL_HILITE, [0.4, 0.84, 1.0, 0.55]),
        ] {
            assert_eq!(actual, expected);
        }
    }
}
