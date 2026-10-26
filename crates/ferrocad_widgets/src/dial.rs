//! An unbounded numeric scrubber drawn as a jog dial.
//!
//! A numeric property with a declared range belongs to [`crate::slider`]: the
//! value is a position on a track. A **dimension is different**. `PropertyLength`
//! has no range, so any `min..max` window would be invented, and a position on it
//! would mean nothing. This module is the alternative: a jog dial whose value
//! changes by a **ratio** as the pointer travels.
//!
//! * [`value_for`] is the pure mapping: `48 px` of travel doubles a non-zero
//!   value, in either direction and at any magnitude (0.1 mm and 1 m feel the
//!   same). A drag towards zero reduces the magnitude whatever the sign, so a
//!   negative value rises towards zero when dragged right. A value of exactly
//!   zero cannot be scaled, so it scrubs linearly — one unit per eight px.
//! * [`Dial`] is the gesture: the value it started from, the press position and
//!   the travelled distance. [`Dial::text`] is what the host commits.
//! * [`trigger`] is the small glyph in the row; [`face`] is the expanded control.
//!   Both paint from the [`Dial`] state, so the needle follows the drag and the
//!   ring fills for up to a full turn.
//!
//! There are no limits to clamp: the dial is honest about the property being
//! unbounded, and typing an exact value in the field beside it still works.

use std::f32::consts::{FRAC_PI_2, TAU};

use gpui::prelude::*;
use gpui::{
    App, ClickEvent, IntoElement, PathBuilder, Pixels, Point, SharedString, Window, canvas, div,
    point, px, rgba,
};

use ferrocad_theme::ui::{CHROME_LINE, HOVER_RIM, MUTED, PANEL_ALT};

/// Horizontal travel that doubles a non-zero value.
pub const DOUBLING_PX: f32 = 48.0;
/// The travel of one full needle turn.
pub const TURN_PX: f32 = 96.0;
/// Travel per unit when the drag starts at zero. The ratio map cannot leave zero,
/// so a zero value needs a linear step; one unit per eight px is a usable default
/// for the units a property editor shows (`mm`, `deg`, …).
pub const ZERO_PX_PER_UNIT: f32 = 8.0;
/// The compact trigger's side.
pub const TRIGGER: f32 = 18.0;
/// The expanded face's side.
pub const FACE: f32 = 112.0;

/// The value `start` becomes after `travel` px of horizontal drag.
///
/// The map is multiplicative so the same gesture is useful at every magnitude;
/// the sign only decides which way "more" is. A start of zero falls back to a
/// linear step because no factor can leave zero.
pub fn value_for(start: f64, travel: f32) -> f64 {
    if !start.is_finite() {
        return start;
    }
    if start == 0.0 {
        return f64::from(travel) / f64::from(ZERO_PX_PER_UNIT);
    }
    // Drag right *raises* the value: for a negative start that means shrinking
    // its magnitude, so the exponent flips with the sign.
    let sign = if start < 0.0 { -1.0 } else { 1.0 };
    start * 2f64.powf(sign * f64::from(travel) / f64::from(DOUBLING_PX))
}

/// A jog-dial gesture: where it started and how far the pointer has travelled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dial {
    start: f64,
    press_x: f32,
    travel: f32,
    value: f64,
}

impl Dial {
    /// A gesture that starts at `start` when the pointer is pressed at `press`.
    pub fn new(start: f64, press: Point<Pixels>) -> Self {
        Self {
            start,
            press_x: press.x.0,
            travel: 0.0,
            value: start,
        }
    }

    /// Follow the pointer; returns the value the drag now implies.
    pub fn drag_to(&mut self, at: Point<Pixels>) -> f64 {
        self.travel = at.x.0 - self.press_x;
        self.value = value_for(self.start, self.travel);
        self.value
    }

    /// The value the drag currently implies (the start value until it moves).
    pub fn value(&self) -> f64 {
        self.value
    }

    /// The value the gesture started from.
    pub fn start(&self) -> f64 {
        self.start
    }

    /// Horizontal travel in logical px (positive is right).
    pub fn travel(&self) -> f32 {
        self.travel
    }

    /// Whether the pointer moved far enough to be a scrub rather than a click.
    pub fn changed(&self) -> bool {
        self.travel.abs() >= 0.5
    }

    /// The needle angle in screen coordinates (`-π/2` is up) for the travelled
    /// distance. It is not wrapped: drawing a direction is periodic, so a long
    /// drag keeps turning instead of snapping back.
    pub fn angle(&self) -> f32 {
        -FRAC_PI_2 + self.travel / TURN_PX * TAU
    }

    /// The ring's filled sweep, clamped to one turn in either direction so a long
    /// drag reads as "a full turn" rather than an arc that overlaps itself.
    pub fn sweep(&self) -> f32 {
        (self.travel / TURN_PX).clamp(-1.0, 1.0) * TAU
    }

    /// The editor string to commit, with the row's unit when it has one.
    pub fn text(&self, unit: &str) -> String {
        let value = format_value(self.value);
        if unit.is_empty() {
            value
        } else {
            format!("{value} {unit}")
        }
    }
}

/// A compact editor string with six significant digits.
///
/// The slider's three decimals fit its bounded ranges, but the dial spans
/// magnitudes: rounding `0.0005 mm` to `0` would commit a real loss, and the raw
/// shortest form would expose the last-bit noise of `2f64.powf`. Six significant
/// digits keep what a dimension needs and hide the noise.
pub fn format_value(value: f64) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    let magnitude = value.abs().log10().floor();
    // Extreme exponents would overflow the scaling factor; leave those alone.
    let rounded = if value == 0.0 || magnitude.abs() > 250.0 {
        value
    } else {
        let factor = 10f64.powf(5.0 - magnitude);
        (value * factor).round() / factor
    };
    if rounded == 0.0 {
        return "0".to_string();
    }
    format!("{rounded}")
}

/// The small dial glyph in a property row.
///
/// `dial` is the live gesture when this row's overlay is open (so the needle
/// follows it); `None` draws the resting glyph pointing up.
pub fn trigger(
    key: impl Into<SharedString>,
    dial: Option<Dial>,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let key = key.into();
    let selector = key.clone();
    let open = dial.is_some();
    let angle = dial.map_or(-FRAC_PI_2, |dial| dial.angle());
    let marker = canvas(
        move |_bounds, _window, _cx| {},
        move |bounds, (), window: &mut Window, _cx| {
            paint_trigger(bounds, angle, open, window);
        },
    )
    .size_full();

    div()
        .id(key)
        .debug_selector(move || format!("{selector}-trigger"))
        .w(px(TRIGGER))
        .h(px(TRIGGER))
        .flex_shrink_0()
        .cursor_pointer()
        .child(marker)
        .on_click(on_click)
}

/// The expanded dial face: a ring with ticks, a needle at the drag angle and an
/// accent sweep for the travelled distance.
pub fn face(key: impl Into<SharedString>, dial: Dial) -> impl IntoElement {
    let key = key.into();
    let selector = key.clone();
    let marker = canvas(
        move |_bounds, _window, _cx| {},
        move |bounds, (), window: &mut Window, _cx| {
            paint_face(bounds, dial, window);
        },
    )
    .size_full();

    div()
        .id(key)
        .debug_selector(move || format!("{selector}-face"))
        .w(px(FACE))
        .h(px(FACE))
        .flex_shrink_0()
        .child(marker)
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

fn paint_trigger(bounds: gpui::Bounds<Pixels>, angle: f32, open: bool, window: &mut Window) {
    let centre = bounds.center();
    let radius = (bounds.size.width.0.min(bounds.size.height.0) / 2.0 - 1.0).max(1.0);
    let ink = if open { HOVER_RIM } else { MUTED };

    let mut ring = PathBuilder::stroke(px(1.0));
    ring.add_polygon(&circle(centre, radius, 32), true);
    paint_stroke(window, ring, if open { HOVER_RIM } else { CHROME_LINE });

    let mut needle = PathBuilder::stroke(px(1.5));
    needle.add_polygon(
        &[
            polar(centre, 1.5, angle),
            polar(centre, (radius - 3.5).max(2.0), angle),
        ],
        false,
    );
    paint_stroke(window, needle, ink);

    let mut hub = PathBuilder::fill();
    hub.add_polygon(&circle(centre, 1.5, 12), true);
    paint_fill(window, hub, ink);
}

fn paint_face(bounds: gpui::Bounds<Pixels>, dial: Dial, window: &mut Window) {
    let centre = bounds.center();
    let radius = (bounds.size.width.0.min(bounds.size.height.0) / 2.0 - 2.0).max(4.0);
    let dragging = dial.changed();

    let mut rim = PathBuilder::stroke(px(1.0));
    rim.add_polygon(&circle(centre, radius, 72), true);
    paint_stroke(window, rim, if dragging { HOVER_RIM } else { CHROME_LINE });

    // Twelve ticks; the quarters are longer so the eye can find "up".
    let mut ticks = PathBuilder::stroke(px(1.0));
    for index in 0..12 {
        let angle = -FRAC_PI_2 + index as f32 * TAU / 12.0;
        let long = index % 3 == 0;
        let inner = radius - if long { 8.0 } else { 4.0 };
        ticks.add_polygon(
            &[
                polar(centre, inner, angle),
                polar(centre, radius - 1.5, angle),
            ],
            false,
        );
    }
    paint_stroke(window, ticks, CHROME_LINE);

    // The travelled distance, up to one turn.
    if dragging {
        let mut trail = PathBuilder::stroke(px(2.5));
        trail.add_polygon(
            &arc(
                centre,
                radius - 12.0,
                -FRAC_PI_2,
                -FRAC_PI_2 + dial.sweep(),
                48,
            ),
            false,
        );
        paint_stroke(window, trail, HOVER_RIM);
    }

    let mut needle = PathBuilder::stroke(px(2.0));
    needle.add_polygon(
        &[
            polar(centre, 7.0, dial.angle()),
            polar(centre, radius - 16.0, dial.angle()),
        ],
        false,
    );
    paint_stroke(window, needle, if dragging { HOVER_RIM } else { MUTED });

    let mut knob = PathBuilder::fill();
    knob.add_polygon(&circle(centre, 6.0, 24), true);
    paint_fill(window, knob, PANEL_ALT);
    let mut knob_rim = PathBuilder::stroke(px(1.0));
    knob_rim.add_polygon(&circle(centre, 6.0, 24), true);
    paint_stroke(
        window,
        knob_rim,
        if dragging { HOVER_RIM } else { CHROME_LINE },
    );
}

fn paint_fill(window: &mut Window, builder: PathBuilder, color: u32) {
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgba(color));
    }
}

fn paint_stroke(window: &mut Window, builder: PathBuilder, color: u32) {
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgba(color));
    }
}

fn polar(centre: Point<Pixels>, radius: f32, angle: f32) -> Point<Pixels> {
    point(
        px(centre.x.0 + radius * angle.cos()),
        px(centre.y.0 + radius * angle.sin()),
    )
}

fn circle(centre: Point<Pixels>, radius: f32, segments: usize) -> Vec<Point<Pixels>> {
    (0..segments)
        .map(|index| polar(centre, radius, TAU * index as f32 / segments as f32))
        .collect()
}

fn arc(
    centre: Point<Pixels>,
    radius: f32,
    from: f32,
    to: f32,
    segments: usize,
) -> Vec<Point<Pixels>> {
    (0..=segments)
        .map(|index| {
            let angle = from + (to - from) * index as f32 / segments as f32;
            polar(centre, radius, angle)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Modifiers, Render, TestAppContext, VisualTestContext, div};

    #[test]
    fn a_drag_doubles_per_scale_step_in_either_direction() {
        assert_eq!(value_for(10.0, 0.0), 10.0);
        assert!((value_for(10.0, DOUBLING_PX) - 20.0).abs() < 1e-9);
        assert!((value_for(10.0, -DOUBLING_PX) - 5.0).abs() < 1e-9);
        assert!((value_for(10.0, 10.0 * DOUBLING_PX) - 10.0 * 1024.0).abs() < 1e-6);
    }

    #[test]
    fn a_negative_value_rises_towards_zero_when_dragged_right() {
        assert!((value_for(-10.0, DOUBLING_PX) + 5.0).abs() < 1e-9);
        assert!((value_for(-10.0, -DOUBLING_PX) + 20.0).abs() < 1e-9);
    }

    #[test]
    fn zero_scrubs_linearly_in_both_directions() {
        assert!((value_for(0.0, ZERO_PX_PER_UNIT) - 1.0).abs() < 1e-9);
        assert!((value_for(0.0, -ZERO_PX_PER_UNIT) + 1.0).abs() < 1e-9);
        assert_eq!(value_for(0.0, 0.0), 0.0);
    }

    #[test]
    fn the_dial_reports_its_travel_and_commit_text() {
        let mut dial = Dial::new(10.0, point(px(0.0), px(0.0)));
        assert!(!dial.changed());
        assert_eq!(dial.value(), 10.0);
        assert!((dial.drag_to(point(px(DOUBLING_PX), px(0.0))) - 20.0).abs() < 1e-9);
        assert!(dial.changed());
        assert_eq!(dial.text("mm"), "20 mm");
        assert_eq!(dial.text(""), "20");
        // Half a turn is a half turn of the needle; the sweep never exceeds one.
        assert!((dial.angle() - FRAC_PI_2).abs() < 1e-6);
        assert!((dial.sweep() - TAU / 2.0).abs() < 1e-6);
        dial.drag_to(point(px(10.0 * TURN_PX), px(0.0)));
        assert!((dial.sweep() - TAU).abs() < 1e-6);
    }

    #[test]
    fn the_commit_text_keeps_six_significant_digits() {
        assert_eq!(format_value(20.0), "20");
        assert_eq!(format_value(0.000507), "0.000507");
        assert_eq!(format_value(0.1 + 0.2), "0.3");
        assert_eq!(format_value(123456.7), "123457");
        assert_eq!(format_value(-0.0), "0");
        assert_eq!(format_value(10.0 * 2f64.powf(1.0 / 3.0)), "12.5992");
    }

    #[test]
    fn a_non_finite_start_is_left_alone() {
        assert!(value_for(f64::NAN, DOUBLING_PX).is_nan());
        assert_eq!(value_for(f64::INFINITY, DOUBLING_PX), f64::INFINITY);
    }

    struct Harness;

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(trigger("dial-test", None, |_event, _window, _cx| {}))
                .child(face("dial-test", Dial::new(10.0, point(px(0.0), px(0.0)))))
        }
    }

    #[gpui::test]
    fn the_trigger_and_face_render(cx: &mut TestAppContext) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_window, cx| cx.new(|_| Harness))
                .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let trigger = visual
            .debug_bounds("dial-test-trigger")
            .expect("the trigger is rendered");
        assert_eq!(trigger.size.width, px(TRIGGER));
        let face = visual
            .debug_bounds("dial-test-face")
            .expect("the face is rendered");
        assert_eq!(face.size.width, px(FACE));

        // A click on the trigger reaches its host callback.
        visual.simulate_click(trigger.center(), Modifiers::default());
    }
}
