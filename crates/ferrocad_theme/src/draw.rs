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
