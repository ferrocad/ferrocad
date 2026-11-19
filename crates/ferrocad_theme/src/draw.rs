//! The tokens at the draw contract: **linear** RGBA for the renderer.
//!
//! Colours are authored as sRGB hex and consumed by the renderer as linear RGBA.
//! [`linear_from_srgb_hex`] is the one place that conversion happens for these
//! tokens; the gpui chrome uses the packed sRGB table in [`crate::ui`].

/// Convert an `0xRRGGBB` sRGB colour to linear RGBA with alpha `1.0`.
///
/// Uses the IEC 61966-2-1 transfer function, the conversion WebGPU's
/// `Rgba8UnormSrgb` target applies on write.
pub fn linear_from_srgb_hex(hex: u32) -> [f32; 4] {
    fn channel(value: u32) -> f32 {
        let c = (value & 0xff) as f32 / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }
    [
        channel(hex >> 16),
        channel(hex >> 8),
        channel(hex),
        1.0,
    ]
}

/// Convert linear RGBA back to an sRGB `0xRRGGBB`, ignoring alpha.
///
/// This is the inverse of [`linear_from_srgb_hex`]; the round-trip test uses it to
/// prove each draw token still names the hex it was authored from.
pub fn srgb_hex_from_linear(linear: [f32; 4]) -> u32 {
    fn channel(value: f32) -> u32 {
        let c = if value <= 0.003_130_8 {
            value * 12.92
        } else {
            1.055 * value.powf(1.0 / 2.4) - 0.055
        };
        (c * 255.0).round().clamp(0.0, 255.0) as u32
    }
    (channel(linear[0]) << 16) | (channel(linear[1]) << 8) | channel(linear[2])
}

/// Canvas void (`#0F1218`), the top of the scene gradient and the clear colour.
pub const CANVAS_VOID: [f32; 4] = [0.004777, 0.006049, 0.009134, 1.0];

/// Canvas void far (`#161B22`), the bottom of the scene gradient.
pub const CANVAS_FAR: [f32; 4] = [0.008023, 0.010960, 0.015996, 1.0];

/// Glass-shade composite (`#151A24`): side and downward-facing entity facets.
pub const GLASS_SHADE: [f32; 4] = [0.007499, 0.010330, 0.017642, 1.0];

/// Glass-top composite (`#555C67`): upward-facing entity facets (V2b threshold).
pub const GLASS_TOP: [f32; 4] = [0.090842, 0.107023, 0.135633, 1.0];

/// Front entity edge: `#D6E0F0` at `0.62`.
pub const EDGE_FRONT: [f32; 4] = [0.672443, 0.745404, 0.871367, 0.62];

/// Occluded entity edge: `#D6E0F0` at `0.10`.
pub const EDGE_OCCLUDED: [f32; 4] = [0.672443, 0.745404, 0.871367, 0.10];

/// The alpha multiplier for a ghost pass (the occluded-edge token's alpha).
pub const GHOST_ALPHA: f32 = 0.10;

// -- Operation and preview accents -----------------------------------------

/// The grid and axis **default ink** (`#94A3B8`, linear here): what the native grid and
/// the axis cross draw when the settings have no override, and what a provider overlay
/// receives as its starting colour.
///
/// This is a default, not a law: `FerroCAD.Grid.color` / `FerroCAD.Axis.color_*` are
/// the user's settings (FerroCAD extensions, not upstream names) and win when set. The
/// token exists so the two backends, both editions and the Python defaults cannot drift
/// apart.
pub const GRID_INK: [f32; 4] = [0.296138, 0.366253, 0.479320, 1.0];

/// The X axis default ink (`#F87171`, linear here).
pub const AXIS_X: [f32; 4] = [0.938686, 0.165132, 0.165132, 1.0];

/// The Y axis default ink (`#4ADE80`, linear here).
pub const AXIS_Y: [f32; 4] = [0.068478, 0.730461, 0.215861, 1.0];

/// The Z axis default ink (`#38BDF8`, linear here).
pub const AXIS_Z: [f32; 4] = [0.039546, 0.508881, 0.938686, 1.0];

/// A tool's **preview ink** (`#A78BFA`, linear here): the ghost's perimeter, the
/// in-progress band and the anchor marker.
///
/// One token covers the creation and the modify ghosts, and it owns an accent no other
/// state uses — selection is cyan, additive amber, subtractive red — so "not geometry
/// yet" reads as its own state rather than borrowing the operation's colour. The ghost's
/// fill is the surface glass; this is only its perimeter.
pub const PREVIEW_INK: [f32; 4] = [0.386429, 0.258183, 0.955973, 1.0];

/// Additive operations (`#F59E0B`, linear here): the push/pull stem, an extrude or pad.
///
/// The renderer-side twin of [`crate::ui::PUSH_PULL_COLOR`]; the stem and the wheel's
/// additive rows share the one accent.
pub const ADDITIVE_INK: [f32; 4] = [0.913099, 0.341914, 0.003347, 1.0];

/// Subtractive operations (`#EF4444`, linear here): the cut badge, a pocket or delete.
///
/// The renderer-side twin of [`crate::ui::SUBTRACT_COLOR`].
pub const SUBTRACTIVE_INK: [f32; 4] = [0.863157, 0.057805, 0.057805, 1.0];
