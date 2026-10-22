//! The visual-language tokens at the draw contract.
//!
//! Colours are authored as sRGB hex and consumed by the renderer as **linear** RGBA.
//! This module is the one place the conversion happens for the GPU-side tokens; the
//! gpui chrome keeps packed sRGB (`0xRRGGBBAA`) because that is what gpui binds.
//!
//! The full token crate is V0 of `docs/interaction/visual-language.md`; this module is
//! the leaf table those slices consume until that crate exists. A token is a constant
//! here, not a literal at a call site.

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

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: [f32; 4], expected: [f32; 4]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!(
                (actual - expected).abs() < 1e-6,
                "{actual} != {expected}"
            );
        }
    }

    #[test]
    fn the_canvas_tokens_are_the_linear_forms_of_their_hex() {
        close(linear_from_srgb_hex(0x0f1218), CANVAS_VOID);
        close(linear_from_srgb_hex(0x161b22), CANVAS_FAR);
    }

    #[test]
    fn the_glass_tokens_are_the_linear_forms_of_their_hex() {
        close(linear_from_srgb_hex(0x151a24), GLASS_SHADE);
        close(linear_from_srgb_hex(0x555c67), GLASS_TOP);
    }

    #[test]
    fn the_edge_tokens_share_the_front_rgb_and_differ_only_in_alpha() {
        let linear = linear_from_srgb_hex(0xd6e0f0);
        close(
            EDGE_FRONT,
            [linear[0], linear[1], linear[2], 0.62],
        );
        close(
            EDGE_OCCLUDED,
            [linear[0], linear[1], linear[2], 0.10],
        );
        assert_eq!(GHOST_ALPHA, EDGE_OCCLUDED[3]);
    }
}
