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

use ferrocad_theme::ui::{AXIS_X, AXIS_Y, AXIS_Z, HOVER_RIM, MUTED};

/// The bug glyph's side.
pub const BUG: f32 = 16.0;

/// A control glyph's side in the header, in logical px.
pub const CONTROL: f32 = 14.0;

/// The 16-unit design box every painted glyph uses.
const DESIGN: f32 = 16.0;

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

/// A 4 × 4 reference grid.
pub fn grid(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, active, paint_grid)
}

/// A three-axis world triad in the axis tokens.
pub fn axes(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, active, paint_axes)
}

/// A plane in perspective with a normal arrow.
pub fn workplane(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, active, paint_workplane)
}

/// The transform gizmo: an axis triad with grab handles.
pub fn gizmo(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, active, paint_gizmo)
}

/// A magnet with a small lock, the global snap switch.
pub fn snap(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    glyph(key, active, paint_snap)
}

/// A presentational control glyph. The host owns the click target, hover and tooltip.
fn glyph(
    key: impl Into<SharedString>,
    active: bool,
    paint: fn(gpui::Bounds<Pixels>, bool, &mut Window),
) -> impl IntoElement {
    let key = key.into();
    let selector = key.clone();
    let marker = canvas(
        move |_bounds, _window, _cx| {},
        move |bounds, (), window: &mut Window, _cx| {
            paint(bounds, active, window);
        },
    )
    .size_full();

    div()
        .id(key)
        .debug_selector(move || format!("{selector}-icon"))
        .w(px(CONTROL))
        .h(px(CONTROL))
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

fn ink(active: bool) -> u32 {
    if active { HOVER_RIM } else { MUTED }
}

/// Map a 16-unit design coordinate to the glyph's box.
fn scaler(bounds: gpui::Bounds<Pixels>) -> impl Fn(f32, f32) -> Point<Pixels> {
    let centre = bounds.center();
    let scale = bounds.size.width.0.min(bounds.size.height.0) / DESIGN;
    move |x, y| {
        point(
            px(centre.x.0 + (x - DESIGN / 2.0) * scale),
            px(centre.y.0 + (y - DESIGN / 2.0) * scale),
        )
    }
}

fn paint_grid(bounds: gpui::Bounds<Pixels>, active: bool, window: &mut Window) {
    let at = scaler(bounds);
    let mut path = PathBuilder::stroke(px(1.0));
    path.add_polygon(
        &[at(2.5, 2.5), at(13.5, 2.5), at(13.5, 13.5), at(2.5, 13.5)],
        true,
    );
    for value in [5.25, 8.0, 10.75] {
        path.add_polygon(&[at(value, 2.5), at(value, 13.5)], false);
        path.add_polygon(&[at(2.5, value), at(13.5, value)], false);
    }
    paint_stroke(window, path, ink(active));
}

fn paint_axes(bounds: gpui::Bounds<Pixels>, active: bool, window: &mut Window) {
    let at = scaler(bounds);
    let (x_col, y_col, z_col) = if active {
        (AXIS_X, AXIS_Y, AXIS_Z)
    } else {
        (MUTED, MUTED, MUTED)
    };

    let mut x = PathBuilder::stroke(px(1.25));
    x.add_polygon(&[at(4.5, 11.5), at(13.0, 11.5)], false);
    x.add_polygon(&[at(13.0, 11.5), at(11.4, 10.9)], false);
    x.add_polygon(&[at(13.0, 11.5), at(11.4, 12.1)], false);
    paint_stroke(window, x, x_col);

    let mut y = PathBuilder::stroke(px(1.25));
    y.add_polygon(&[at(4.5, 11.5), at(4.5, 3.0)], false);
    y.add_polygon(&[at(4.5, 3.0), at(3.9, 4.6)], false);
    y.add_polygon(&[at(4.5, 3.0), at(5.1, 4.6)], false);
    paint_stroke(window, y, y_col);

    let mut z = PathBuilder::stroke(px(1.25));
    z.add_polygon(&[at(4.5, 11.5), at(10.5, 6.5)], false);
    z.add_polygon(&[at(10.5, 6.5), at(9.2, 7.2)], false);
    z.add_polygon(&[at(10.5, 6.5), at(10.8, 8.1)], false);
    paint_stroke(window, z, z_col);
}

fn paint_workplane(bounds: gpui::Bounds<Pixels>, active: bool, window: &mut Window) {
    let at = scaler(bounds);
    let mut path = PathBuilder::stroke(px(1.25));
    path.add_polygon(
        &[at(3.0, 10.5), at(8.5, 6.0), at(13.0, 8.0), at(7.5, 12.5)],
        true,
    );
    path.add_polygon(&[at(8.0, 9.1), at(8.0, 5.6)], false);
    path.add_polygon(&[at(8.0, 5.6), at(7.4, 6.7)], false);
    path.add_polygon(&[at(8.0, 5.6), at(8.6, 6.7)], false);
    paint_stroke(window, path, ink(active));
}

fn paint_gizmo(bounds: gpui::Bounds<Pixels>, active: bool, window: &mut Window) {
    let at = scaler(bounds);
    let scale = bounds.size.width.0.min(bounds.size.height.0) / DESIGN;
    let handle = ink(active);
    let (x_col, y_col, z_col) = if active {
        (AXIS_X, AXIS_Y, AXIS_Z)
    } else {
        (MUTED, MUTED, MUTED)
    };

    for (x, y, colour) in [(13.0, 11.0, x_col), (5.0, 3.0, y_col), (10.5, 6.5, z_col)] {
        let mut line = PathBuilder::stroke(px(1.25));
        line.add_polygon(&[at(5.0, 11.0), at(x, y)], false);
        paint_stroke(window, line, colour);

        let mut square = PathBuilder::fill();
        square.add_polygon(
            &[
                at(x - 1.1, y - 1.1),
                at(x + 1.1, y - 1.1),
                at(x + 1.1, y + 1.1),
                at(x - 1.1, y + 1.1),
            ],
            true,
        );
        paint_fill(window, square, handle);
    }

    let mut ring = PathBuilder::stroke(px(1.0));
    ring.add_polygon(&ellipse(at(5.0, 11.0), 1.6 * scale, 1.6 * scale, 16), false);
    paint_stroke(window, ring, handle);
}

fn paint_snap(bounds: gpui::Bounds<Pixels>, active: bool, window: &mut Window) {
    let at = scaler(bounds);
    let scale = bounds.size.width.0.min(bounds.size.height.0) / DESIGN;
    let colour = ink(active);

    let mut magnet = PathBuilder::stroke(px(1.25));
    magnet.add_polygon(
        &[
            at(5.0, 3.5),
            at(5.0, 7.4),
            at(5.4, 9.0),
            at(6.3, 10.3),
            at(8.0, 10.9),
            at(9.7, 10.3),
            at(10.6, 9.0),
            at(11.0, 7.4),
            at(11.0, 3.5),
        ],
        false,
    );
    magnet.add_polygon(&[at(5.0, 3.5), at(5.0, 5.2)], false);
    magnet.add_polygon(&[at(11.0, 3.5), at(11.0, 5.2)], false);
    paint_stroke(window, magnet, colour);

    let mut lock = PathBuilder::stroke(px(1.0));
    lock.add_polygon(
        &[
            at(10.4, 11.4),
            at(13.4, 11.4),
            at(13.4, 14.0),
            at(10.4, 14.0),
        ],
        true,
    );
    lock.add_polygon(
        &ellipse(at(11.9, 11.4), 1.1 * scale, 1.1 * scale, 12),
        false,
    );
    paint_stroke(window, lock, colour);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, VisualTestContext};

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
