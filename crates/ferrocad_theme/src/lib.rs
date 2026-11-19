//! The visual-language token table (V0 of `docs/developer/interaction/visual-language.md`).
//!
//! Two representations, one namesake:
//!
//! * [`draw`] — **linear** RGBA, the renderer's draw contract;
//! * [`ui`] — packed sRGB (`0xRRGGBBAA`), what gpui binds.
//!
//! The navigation cube's palette is the third table, [`navicube`]: moved here as-is,
//! mostly sRGB-normalised `[f32; 4]`, not yet normalised onto either representation
//! (`docs/developer/interaction/visual-language.md` §1.6, V0b).
//!
//! A token is a constant here, never a literal at a call site. The tests keep the
//! two representations in step, and [`linear_from_srgb_hex`] /
//! [`srgb_hex_from_linear`] are the round trip between them.
//!
//! The crate is a leaf: no gpui, no wgpu, no kernel. Every GUI crate in the
//! workspace can depend on it without inverting the graph.

pub mod draw;
pub mod navicube;
pub mod ui;

pub use draw::{linear_from_srgb_hex, srgb_hex_from_linear};

#[cfg(test)]
mod tests {
    use crate::draw::{self, linear_from_srgb_hex, srgb_hex_from_linear};
    use crate::ui;

    fn close(actual: [f32; 4], expected: [f32; 4]) {
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
        }
    }

    /// The draw table is the linear form of the authored sRGB hex.
    #[test]
    fn the_draw_tokens_are_the_linear_forms_of_their_hex() {
        close(linear_from_srgb_hex(0x0f1218), draw::CANVAS_VOID);
        close(linear_from_srgb_hex(0x161b22), draw::CANVAS_FAR);
        close(linear_from_srgb_hex(0x151a24), draw::GLASS_SHADE);
        close(linear_from_srgb_hex(0x555c67), draw::GLASS_TOP);
    }

    /// The round trip: each draw token returns to the hex it was authored from, and
    /// the hex converts back to the same RGB. Alpha is a per-token choice (an edge
    /// carries 0.62), so it is not part of the hex round trip.
    #[test]
    fn every_draw_token_round_trips_to_its_hex() {
        for (linear, hex) in [
            (draw::CANVAS_VOID, 0x0f1218),
            (draw::CANVAS_FAR, 0x161b22),
            (draw::GLASS_SHADE, 0x151a24),
            (draw::GLASS_TOP, 0x555c67),
            (draw::EDGE_FRONT, 0xd6e0f0),
            (draw::EDGE_OCCLUDED, 0xd6e0f0),
            (draw::GRID_INK, 0x94a3b8),
            (draw::AXIS_X, 0xf87171),
            (draw::AXIS_Y, 0x4ade80),
            (draw::AXIS_Z, 0x38bdf8),
            (draw::PREVIEW_INK, 0xa78bfa),
            (draw::ADDITIVE_INK, 0xf59e0b),
            (draw::SUBTRACTIVE_INK, 0xef4444),
        ] {
            assert_eq!(srgb_hex_from_linear(linear), hex);
            let back = linear_from_srgb_hex(hex);
            close(
                [linear[0], linear[1], linear[2], back[3]],
                back,
            );
        }
    }

    /// The edge tokens share the front RGB and differ only in alpha.
    #[test]
    fn the_edge_tokens_share_the_front_rgb_and_differ_only_in_alpha() {
        let linear = linear_from_srgb_hex(0xd6e0f0);
        close(
            draw::EDGE_FRONT,
            [linear[0], linear[1], linear[2], 0.62],
        );
        close(
            draw::EDGE_OCCLUDED,
            [linear[0], linear[1], linear[2], 0.10],
        );
        assert_eq!(draw::GHOST_ALPHA, draw::EDGE_OCCLUDED[3]);
    }

    /// A UI token is packed sRGB: opaque `0xRRGGBB`, translucent `0xRRGGBBAA`.
    #[test]
    fn the_ui_tokens_are_packed_srgb() {
        assert_eq!(ui::FG, 0xd8dbe0);
        assert_eq!(ui::MUTED, 0x8b919a);
        assert_eq!(ui::DOT_INACTIVE, 0x8b919a4d, "MUTED at 30%");
        assert_eq!(ui::ROW_HOVER, 0xffffff14, "8% white, the hairline family");
        assert_eq!(ui::CHROME_LINE, 0xffffff14);
        assert_eq!(ui::ACTIVE_ACCENT, 0x38bdf8);
        assert_eq!(ui::CHROME_ACTIVE, 0x67d7ff);
        assert_eq!(ui::DOCK_BG, 0x16181de6);
        assert_eq!(ui::DOCK_BORDER, 0xffffff1f);
        assert_eq!(ui::CHROME_ACTIVE_BG, 0x1b2b3855, "the active chrome fill");
    }

    /// Where the two tables name the same colour, they agree.
    #[test]
    fn the_ui_and_draw_tables_agree_on_shared_colours() {
        // The glass is the one colour authored in both representations: the pill's
        // sRGB tint and the renderer's linear shade.
        let glass = linear_from_srgb_hex((ui::GLASS & 0xffffff00) >> 8);
        close(glass, draw::GLASS_SHADE);
        assert_eq!(srgb_hex_from_linear(draw::GLASS_SHADE), 0x151a24);

        // The operation accent and the preview ink are now tokens on both sides, so the
        // ghost, the widget and the stem cannot drift apart again.
        for (linear, hex) in [
            (draw::GRID_INK, ui::GRID_INK),
            (draw::AXIS_X, ui::AXIS_X),
            (draw::AXIS_Y, ui::AXIS_Y),
            (draw::AXIS_Z, ui::AXIS_Z),
            (draw::PREVIEW_INK, ui::PREVIEW_INK),
            (draw::ADDITIVE_INK, ui::PUSH_PULL_COLOR),
            (draw::SUBTRACTIVE_INK, ui::SUBTRACT_COLOR),
        ] {
            close(linear, linear_from_srgb_hex(hex));
            assert_eq!(srgb_hex_from_linear(linear), hex);
        }
    }
}
