//! Embedded SVG pictograms for chrome that has no room for a text label.
//!
//! The assets are the outline glyphs selected in
//! `docs/developer/interaction/iconography.md` (the IconMCP set, plus the authored
//! `workplane`); they are embedded with [`include_bytes!`] and rendered by gpui's
//! SVG element, so there is no asset source, filesystem lookup or network dependency. The renderer produces an alpha
//! mask and `text_color` applies the ink: greyscale when off (`MUTED`), accent when
//! on (`HOVER_RIM`).
//!
//! The element is **presentational**: it draws the glyph and marks `active` with the
//! accent. The host owns the click target, its hover and its tooltip.

use gpui::{IntoElement, SharedString, div, prelude::*, px, rgb, svg};

use ferrocad_theme::ui::{HOVER_RIM, MUTED};

/// The Dev Tools bug's side.
pub const BUG: f32 = 16.0;

/// A header control glyph's side.
pub const CONTROL: f32 = 16.0;

const BUG_SVG: &[u8] = include_bytes!("../assets/icons/bug.svg");
const GRID_SVG: &[u8] = include_bytes!("../assets/icons/grid.svg");
const AXES_SVG: &[u8] = include_bytes!("../assets/icons/axes.svg");
const WORKPLANE_SVG: &[u8] = include_bytes!("../assets/icons/workplane.svg");
const GIZMO_SVG: &[u8] = include_bytes!("../assets/icons/gizmo.svg");
const SNAP_SVG: &[u8] = include_bytes!("../assets/icons/snap.svg");

/// The literal bug that opens the Dev Tools window.
pub fn bug(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, BUG, "bug", active, BUG_SVG)
}

/// The 4 × 4 reference grid.
pub fn grid(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, CONTROL, "icon", active, GRID_SVG)
}

/// The three-axis world triad.
pub fn axes(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, CONTROL, "icon", active, AXES_SVG)
}

/// A plane in perspective with a normal arrow.
pub fn workplane(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, CONTROL, "icon", active, WORKPLANE_SVG)
}

/// The transform gizmo: the axis triad with a grab hand.
pub fn gizmo(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, CONTROL, "icon", active, GIZMO_SVG)
}

/// The global snap switch: a magnet with a lock.
pub fn snap(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, CONTROL, "icon", active, SNAP_SVG)
}

/// A presentational glyph. The host owns the click target, hover and tooltip.
fn glyph(
    key: impl Into<SharedString>,
    size: f32,
    suffix: &'static str,
    active: bool,
    data: &'static [u8],
) -> impl IntoElement {
    let key = key.into();
    let selector = key.clone();
    div()
        .id(key)
        .debug_selector(move || format!("{selector}-{suffix}"))
        .w(px(size))
        .h(px(size))
        .child(
            svg()
                .data(data)
                .w_full()
                .h_full()
                .text_color(rgb(if active { HOVER_RIM } else { MUTED })),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, VisualTestContext, Window};

    struct Harness;

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .child(bug("bug-test", false))
                .child(grid("grid-test", false))
                .child(axes("axis-test", true))
                .child(workplane("wp-test", true))
                .child(gizmo("gizmo-test", false))
                .child(snap("snap-test", true))
        }
    }

    #[test]
    fn the_icon_assets_are_embedded() {
        for (name, data) in [
            ("bug", BUG_SVG),
            ("grid", GRID_SVG),
            ("axes", AXES_SVG),
            ("workplane", WORKPLANE_SVG),
            ("gizmo", GIZMO_SVG),
            ("snap", SNAP_SVG),
        ] {
            assert!(data.starts_with(b"<svg"), "{name} is an SVG");
            assert!(
                data.windows(6).any(|window| window == b"stroke".as_slice()),
                "{name} carries strokes"
            );
        }
    }

    #[gpui::test]
    fn the_bug_renders_at_its_side(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_window, cx| cx.new(|_| Harness))
                .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
        let bounds = visual
            .debug_bounds("bug-test-bug")
            .expect("the bug is painted");
        assert_eq!(bounds.size.width, px(BUG));
        assert_eq!(bounds.size.height, px(BUG));
    }

    #[gpui::test]
    fn the_control_glyphs_render_at_their_side(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_window, cx| cx.new(|_| Harness))
                .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });
        for name in [
            "grid-test-icon",
            "axis-test-icon",
            "wp-test-icon",
            "gizmo-test-icon",
            "snap-test-icon",
        ] {
            let bounds = visual
                .debug_bounds(name)
                .unwrap_or_else(|| panic!("{name} is painted"));
            assert_eq!(bounds.size.width, px(CONTROL), "{name} width");
            assert_eq!(bounds.size.height, px(CONTROL), "{name} height");
        }
    }
}
