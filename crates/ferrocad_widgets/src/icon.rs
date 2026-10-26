//! Painted pictograms for chrome that has no room for a text label.
//!
//! [`bug`] is the literal bug that opens the Dev Tools window. It is painted from
//! paths rather than set as a Unicode glyph: the default font is not guaranteed to
//! carry an insect, and a painted glyph keeps the same ink and weight as the rest
//! of the chrome at any scale.
//!
//! The element is **presentational**: it draws the bug and marks `active` with the
//! accent. The host owns the click target, its hover and its tooltip.

use gpui::prelude::*;
use gpui::{
    IntoElement, PathBuilder, Pixels, Point, SharedString, Window, canvas, div, point, px, rgba,
};

use ferrocad_theme::ui::{HOVER_RIM, MUTED};

/// The bug glyph's side.
pub const BUG: f32 = 16.0;

/// A literal bug: an oval body, a head, six legs and two antennae.
///
/// `active` inks it with the hover accent (the window it opens is already open);
/// otherwise it uses the muted chrome ink.
pub fn bug(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    let key = key.into();
    let selector = key.clone();
    let marker = canvas(
        move |_bounds, _window, _cx| {},
        move |bounds, (), window: &mut Window, _cx| {
            paint_bug(bounds, active, window);
        },
    )
    .size_full();

    div()
        .id(key)
        .debug_selector(move || format!("{selector}-bug"))
        .w(px(BUG))
        .h(px(BUG))
        .child(marker)
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

/// Paint the bug into `bounds`. Coordinates are `16 × 16` and scale with the box.
fn paint_bug(bounds: gpui::Bounds<Pixels>, active: bool, window: &mut Window) {
    let ink = if active { HOVER_RIM } else { MUTED };
    let centre = bounds.center();
    let scale = bounds.size.width.0.min(bounds.size.height.0) / BUG;
    let at = |dx: f32, dy: f32| point(px(centre.x.0 + dx * scale), px(centre.y.0 + dy * scale));

    // Body: a soft oval, tinted just enough to read as a silhouette.
    let mut body = PathBuilder::fill();
    body.add_polygon(&ellipse(at(0.0, 1.4), 3.3 * scale, 4.6 * scale, 32), true);
    paint_fill_alpha(window, body, ink, 0x2e);
    let mut rim = PathBuilder::stroke(px(1.0));
    rim.add_polygon(&ellipse(at(0.0, 1.4), 3.3 * scale, 4.6 * scale, 32), true);
    paint_stroke(window, rim, ink);

    // The wing seam and the six legs.
    let mut seams = PathBuilder::stroke(px(1.0));
    seams.add_polygon(&[at(0.0, -2.6), at(0.0, 5.4)], false);
    for (index, dy) in [-1.6_f32, 1.2, 4.0].into_iter().enumerate() {
        let bend = [-0.7_f32, 0.0, 0.7][index];
        seams.add_polygon(&[at(-2.7, dy), at(-6.0, dy + bend)], false);
        seams.add_polygon(&[at(2.7, dy), at(6.0, dy + bend)], false);
    }
    paint_stroke(window, seams, ink);

    // Head and antennae.
    let mut head = PathBuilder::fill();
    head.add_polygon(&ellipse(at(0.0, -4.0), 1.9 * scale, 1.9 * scale, 24), true);
    paint_fill(window, head, ink);
    let mut antennae = PathBuilder::stroke(px(1.0));
    antennae.add_polygon(&[at(-1.1, -5.4), at(-3.0, -7.2)], false);
    antennae.add_polygon(&[at(1.1, -5.4), at(3.0, -7.2)], false);
    paint_stroke(window, antennae, ink);
}

fn ellipse(centre: Point<Pixels>, rx: f32, ry: f32, segments: usize) -> Vec<Point<Pixels>> {
    (0..segments)
        .map(|index| {
            let angle = std::f32::consts::TAU * index as f32 / segments as f32;
            point(
                px(centre.x.0 + rx * angle.cos()),
                px(centre.y.0 + ry * angle.sin()),
            )
        })
        .collect()
}

fn paint_fill(window: &mut Window, builder: PathBuilder, color: u32) {
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgba(color));
    }
}

fn paint_fill_alpha(window: &mut Window, builder: PathBuilder, color: u32, alpha: u32) {
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgba((color << 8) | alpha));
    }
}

fn paint_stroke(window: &mut Window, builder: PathBuilder, color: u32) {
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgba(color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, VisualTestContext};

    struct Harness;

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().child(bug("bug-test", false))
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
}
