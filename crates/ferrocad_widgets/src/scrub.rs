//! An unbounded numeric scrubber: drag the handle, read the scale.
//!
//! A numeric property with a declared range belongs to [`crate::slider`]: the value
//! is a position on a track. A **dimension is different**. `PropertyLength` has no
//! range, so any `min..max` window would be invented, and a position on it would
//! mean nothing.
//!
//! This module is the alternative. The row's [`handle`] is dragged **directly**:
//! vertical travel changes the value by a ratio (`48 px` doubles it), so the same
//! gesture is useful at 0.1 mm and 1 m. While the button is held, [`scale`] shows
//! the surrounding scale — linear around zero and negatives, logarithmic above
//! zero — with labelled ticks and an accent marker at the current value.
//!
//! * [`Scrub`] is the gesture: the value it started from, the press position and
//!   the travelled distance. [`Scrub::text`] is what the host commits.
//! * [`Scale`] is the pure overlay model: the window shown, its linear/log mapping
//!   and the labelled [`Tick`]s. Nothing here touches the DOM.
//! * [`handle`] and [`scale`] are the painted elements.

use std::f32::consts::TAU;

use gpui::prelude::*;
use gpui::{
    IntoElement, PathBuilder, Pixels, Point, SharedString, Window, canvas, div, point, px, rgb,
    rgba,
};

use ferrocad_theme::ui::{CHROME_LINE, DOCK_BORDER, FG, HOVER_RIM, MUTED, PANEL};

/// Vertical travel that doubles a non-zero value.
pub const DOUBLING_PX: f32 = 48.0;
/// Travel per unit when the scrub starts at zero. The ratio map cannot leave zero,
/// so a zero value needs a linear step: one unit per eight px.
pub const ZERO_PX_PER_UNIT: f32 = 8.0;
/// The handle's side.
pub const HANDLE: f32 = 18.0;
/// The scale overlay's ruler height and panel width.
pub const RULER_HEIGHT: f32 = 148.0;
pub const PANEL_WIDTH: f32 = 176.0;

/// The value `start` becomes after `travel` px of vertical drag (up is positive).
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
    // Drag up *raises* the value: for a negative start that means shrinking its
    // magnitude, so the exponent flips with the sign.
    let sign = if start < 0.0 { -1.0 } else { 1.0 };
    start * 2f64.powf(sign * f64::from(travel) / f64::from(DOUBLING_PX))
}

/// A vertical scrub gesture: where it started and how far the pointer travelled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scrub {
    start: f64,
    press_y: f32,
    travel: f32,
    value: f64,
}

impl Scrub {
    /// A gesture that starts at `start` when the pointer is pressed at `press`.
    pub fn new(start: f64, press: Point<Pixels>) -> Self {
        Self {
            start,
            press_y: press.y.0,
            travel: 0.0,
            value: start,
        }
    }

    /// Follow the pointer; returns the value the drag now implies.
    pub fn drag_to(&mut self, at: Point<Pixels>) -> f64 {
        self.travel = self.press_y - at.y.0;
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

    /// Vertical travel in logical px (positive is up).
    pub fn travel(&self) -> f32 {
        self.travel
    }

    /// Whether the pointer moved far enough to be a scrub rather than a click.
    pub fn changed(&self) -> bool {
        self.travel.abs() >= 0.5
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

/// A labelled position on the scale.
#[derive(Clone, Debug, PartialEq)]
pub struct Tick {
    pub value: f64,
    /// `0` at the bottom of the ruler, `1` at the top.
    pub fraction: f32,
    pub label: String,
}

/// The window of values the overlay shows.
///
/// It is **derived** from the gesture, never declared by the property: a positive
/// value gets a log window four doublings either side of where the scrub started
/// (widened if the drag leaves it), and zero or a negative value gets a linear
/// window around the pair. The window always contains the current value, so the
/// marker is always visible.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    pub min: f64,
    pub max: f64,
    pub log: bool,
}

impl Scale {
    /// The window for a scrub that started at `start` and now holds `value`.
    pub fn around(start: f64, value: f64) -> Self {
        if start > 0.0 && value > 0.0 {
            let low = (start / 16.0).min(value / 2.0).max(f64::MIN_POSITIVE);
            let high = (start * 16.0).max(value * 2.0);
            return Self {
                min: low,
                max: high,
                log: true,
            };
        }
        let lo = start.min(value);
        let hi = start.max(value);
        if hi <= 0.0 {
            // Negative, or both at zero: a padded linear window.
            let pad = ((hi - lo) * 0.25).max(1.0);
            return Self {
                min: lo - pad,
                max: hi + pad,
                log: false,
            };
        }
        if lo >= 0.0 {
            // Zero start moving up: keep zero as the floor so growth is legible.
            return Self {
                min: 0.0,
                max: hi.max(1.0) * 1.25,
                log: false,
            };
        }
        let pad = ((hi - lo) * 0.25).max(1.0);
        Self {
            min: lo - pad,
            max: hi + pad,
            log: false,
        }
    }

    /// The marker position for `value`, `0` at the bottom and `1` at the top.
    pub fn fraction(&self, value: f64) -> f32 {
        let span = self.max - self.min;
        if !(span > 0.0) {
            return 0.5;
        }
        let position = if self.log {
            (value.max(self.min).ln() - self.min.ln()) / (self.max.ln() - self.min.ln())
        } else {
            (value - self.min) / span
        };
        position.clamp(0.0, 1.0) as f32
    }

    /// The labelled ticks inside the window.
    ///
    /// A log window uses `1`, `2` and `5` per decade; a linear window uses a
    /// `1/2/5` step sized for about five labels. At most twelve are returned, so a
    /// wide window never crowds the ruler.
    pub fn ticks(&self) -> Vec<Tick> {
        let mut ticks = Vec::new();
        if self.log {
            let low = self.min.log10().floor() as i32;
            let high = self.max.log10().ceil() as i32;
            for exponent in low..=high {
                for multiple in [1.0, 2.0, 5.0] {
                    let value = multiple * 10f64.powi(exponent);
                    if value >= self.min && value <= self.max {
                        ticks.push(self.tick(value));
                    }
                }
            }
        } else {
            let step = nice_step((self.max - self.min) / 4.0);
            let mut value = (self.min / step).ceil() * step;
            while value <= self.max && ticks.len() < 12 {
                ticks.push(self.tick(value));
                value += step;
            }
        }
        if ticks.len() > 12 {
            let stride = ticks.len().div_ceil(12);
            ticks = ticks.into_iter().step_by(stride).collect();
        }
        ticks
    }

    fn tick(&self, value: f64) -> Tick {
        Tick {
            value,
            fraction: self.fraction(value),
            label: tick_label(value),
        }
    }
}

/// A compact editor string with six significant digits.
///
/// The slider's three decimals fit its bounded ranges, but a scrub spans
/// magnitudes: rounding `0.0005 mm` to `0` would commit a real loss, and the raw
/// shortest form would expose the last-bit noise of `2f64.powf`.
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

/// A ruler label: three decimals, or scientific notation outside `1e-3 .. 1e5`.
fn tick_label(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    let magnitude = value.abs();
    if !(1e-3..1e5).contains(&magnitude) {
        return format!("{value:.1e}");
    }
    let mut text = format!("{value:.3}");
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if text == "-0" { "0".to_string() } else { text }
}

/// A `1`, `2` or `5` times a power of ten, at least as large as `raw`.
fn nice_step(raw: f64) -> f64 {
    if !(raw > 0.0) {
        return 1.0;
    }
    let exponent = raw.log10().floor();
    let base = 10f64.powf(exponent);
    let fraction = raw / base;
    let multiple = if fraction <= 1.0 {
        1.0
    } else if fraction <= 2.0 {
        2.0
    } else if fraction <= 5.0 {
        5.0
    } else {
        10.0
    };
    multiple * base
}

/// The drag handle in a property row: a double vertical arrow.
///
/// It is presentational; the host starts the gesture with `on_mouse_down` and
/// forwards the moves from its window root (so a drag that leaves the handle keeps
/// updating). `active` inks it with the accent while the scrub is running.
pub fn handle(key: impl Into<SharedString>, active: bool) -> impl IntoElement {
    let key = key.into();
    let selector = key.clone();
    let marker = canvas(
        move |_bounds, _window, _cx| {},
        move |bounds, (), window: &mut Window, _cx| {
            paint_handle(bounds, active, window);
        },
    )
    .size_full();

    div()
        .id(key)
        .debug_selector(move || format!("{selector}-handle"))
        .w(px(HANDLE))
        .h(px(HANDLE))
        .child(marker)
}

/// The scale overlay shown while a scrub is dragging.
///
/// `label` is the row's name and `scrub` its live gesture; the header carries the
/// readout, the ruler the ticks and the marker.
pub fn scale(
    key: impl Into<SharedString>,
    label: &str,
    scrub: Scrub,
    unit: &str,
) -> impl IntoElement {
    let key = key.into();
    let selector = key.clone();
    let model = Scale::around(scrub.start(), scrub.value());
    let ticks = model.ticks();
    let marker = model.fraction(scrub.value());
    let readout = scrub.text(unit);

    let painted = ticks.clone();
    let ruler = canvas(
        move |_bounds, _window, _cx| {},
        move |bounds, (), window: &mut Window, _cx| {
            paint_ruler(bounds, &painted, marker, window);
        },
    )
    .absolute()
    .size_full();

    let mut body = div().relative().w_full().h(px(RULER_HEIGHT)).child(ruler);
    for tick in &ticks {
        // Keep the label's centre on the tick; the label itself is right-aligned.
        let top = px(((1.0 - tick.fraction) * RULER_HEIGHT - 7.0).clamp(0.0, RULER_HEIGHT - 14.0));
        body = body.child(
            div()
                .absolute()
                .top(top)
                .right_0()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(tick.label.clone()),
        );
    }

    div()
        .id(SharedString::from(format!("{key}-scale")))
        .debug_selector(move || format!("{selector}-scale"))
        .w(px(PANEL_WIDTH))
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded_md()
        // Opaque: the scale is read while the pointer is moving over a busy pane,
        // and a translucent panel made the ticks fight the rows behind it.
        .bg(rgb(PANEL))
        .border_1()
        .border_color(rgba(DOCK_BORDER))
        .text_xs()
        // A press in the overlay must not reach the pane behind it.
        .on_mouse_down(gpui::MouseButton::Left, |_event, _window, cx| {
            cx.stop_propagation()
        })
        .child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .gap_2()
                .text_color(rgb(FG))
                .child(label.to_string())
                .child(div().text_color(rgb(HOVER_RIM)).child(readout)),
        )
        .child(body)
        .child(div().text_color(rgb(MUTED)).child("drag up / down"))
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

fn paint_handle(bounds: gpui::Bounds<Pixels>, active: bool, window: &mut Window) {
    let ink = if active { HOVER_RIM } else { MUTED };
    let centre = bounds.center();
    let scale = bounds.size.width.0.min(bounds.size.height.0) / HANDLE;
    let at = |dx: f32, dy: f32| point(px(centre.x.0 + dx * scale), px(centre.y.0 + dy * scale));

    let mut path = PathBuilder::stroke(px(1.4));
    // The stem.
    path.add_polygon(&[at(0.0, -5.2), at(0.0, 5.2)], false);
    // The arrowheads: up and down.
    path.add_polygon(&[at(-3.0, -2.8), at(0.0, -5.6), at(3.0, -2.8)], false);
    path.add_polygon(&[at(-3.0, 2.8), at(0.0, 5.6), at(3.0, 2.8)], false);
    paint_stroke(window, path, ink);
}

fn paint_ruler(bounds: gpui::Bounds<Pixels>, ticks: &[Tick], marker: f32, window: &mut Window) {
    let left = bounds.left().0 + 8.0;
    let top = bounds.top().0 + 2.0;
    let bottom = bounds.bottom().0 - 2.0;
    let height = (bottom - top).max(1.0);

    let mut track = PathBuilder::stroke(px(1.0));
    track.add_polygon(
        &[point(px(left), px(top)), point(px(left), px(bottom))],
        false,
    );
    let mut marks = PathBuilder::stroke(px(1.0));
    for tick in ticks {
        let y = bottom - height * tick.fraction;
        marks.add_polygon(
            &[point(px(left - 4.0), px(y)), point(px(left + 4.0), px(y))],
            false,
        );
    }
    paint_stroke(window, track, CHROME_LINE);
    paint_stroke(window, marks, CHROME_LINE);

    // The current value: an accent segment across the track and a filled dot.
    let y = bottom - height * marker;
    let mut current = PathBuilder::stroke(px(2.0));
    current.add_polygon(
        &[point(px(left - 4.0), px(y)), point(px(left + 4.0), px(y))],
        false,
    );
    paint_stroke(window, current, HOVER_RIM);
    let mut dot = PathBuilder::fill();
    dot.add_polygon(&ellipse(point(px(left), px(y)), 3.0, 3.0, 16), true);
    paint_fill(window, dot, HOVER_RIM);
}

fn ellipse(centre: Point<Pixels>, rx: f32, ry: f32, segments: usize) -> Vec<Point<Pixels>> {
    (0..segments)
        .map(|index| {
            let angle = TAU * index as f32 / segments as f32;
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

fn paint_stroke(window: &mut Window, builder: PathBuilder, color: u32) {
    if let Ok(path) = builder.build() {
        window.paint_path(path, rgba(color));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Modifiers, Render, TestAppContext, VisualTestContext};

    #[test]
    fn a_scrub_doubles_per_scale_step_in_either_direction() {
        assert_eq!(value_for(10.0, 0.0), 10.0);
        assert!((value_for(10.0, DOUBLING_PX) - 20.0).abs() < 1e-9);
        assert!((value_for(10.0, -DOUBLING_PX) - 5.0).abs() < 1e-9);
        assert!((value_for(10.0, 10.0 * DOUBLING_PX) - 10.0 * 1024.0).abs() < 1e-6);
    }

    #[test]
    fn a_negative_value_rises_towards_zero_when_dragged_up() {
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
    fn the_gesture_reports_its_travel_and_commit_text() {
        let mut scrub = Scrub::new(10.0, point(px(0.0), px(100.0)));
        assert!(!scrub.changed());
        assert_eq!(scrub.value(), 10.0);
        assert!((scrub.drag_to(point(px(0.0), px(100.0 - DOUBLING_PX))) - 20.0).abs() < 1e-9);
        assert!(scrub.changed());
        assert_eq!(scrub.text("mm"), "20 mm");
        assert_eq!(scrub.text(""), "20");
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
    fn a_positive_scale_is_logarithmic_and_contains_the_value() {
        let scale = Scale::around(10.0, 20.0);
        assert!(scale.log);
        assert!(scale.min <= 10.0 && scale.max >= 20.0);
        assert!((scale.fraction(10.0) - 0.5).abs() < 0.2);
        let ticks = scale.ticks();
        assert!(ticks.len() >= 3, "ticks: {ticks:?}");
        assert!(ticks.windows(2).all(|w| w[0].value < w[1].value));
        assert!(ticks.iter().all(|t| (0.0..=1.0).contains(&t.fraction)));
    }

    #[test]
    fn a_zero_start_uses_a_linear_scale_with_zero_as_the_floor() {
        let scale = Scale::around(0.0, 6.0);
        assert!(!scale.log);
        assert_eq!(scale.min, 0.0);
        assert!(scale.max > 6.0);
        assert!(scale.fraction(0.0) < 0.01);
        assert!(scale.fraction(6.0) > 0.7);
    }

    #[test]
    fn a_negative_scale_is_linear_and_contains_both_ends() {
        let scale = Scale::around(-30.0, -15.0);
        assert!(!scale.log);
        assert!(scale.min <= -30.0 && scale.max >= -15.0);
        assert!(scale.fraction(-30.0) < scale.fraction(-15.0));
    }

    #[test]
    fn a_tick_labels_small_values_without_rounding_them_to_zero() {
        assert_eq!(tick_label(0.0001), "1.0e-4");
        assert_eq!(tick_label(0.5), "0.5");
        assert_eq!(tick_label(200000.0), "2.0e5");
    }

    struct Harness {
        scrub: Scrub,
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(handle("scrub-test", true))
                .child(scale("scrub-test", "Length", self.scrub, "mm"))
        }
    }

    #[gpui::test]
    fn the_handle_and_scale_render(cx: &mut TestAppContext) {
        let harness = Harness {
            scrub: Scrub::new(10.0, point(px(0.0), px(0.0))),
        };
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_window, cx| cx.new(|_| harness))
                .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        visual.update(|window, cx| {
            let _ = window.draw(cx);
        });

        let handle = visual
            .debug_bounds("scrub-test-handle")
            .expect("the handle is rendered");
        assert_eq!(handle.size.width, px(HANDLE));
        let scale = visual
            .debug_bounds("scrub-test-scale")
            .expect("the scale is rendered");
        assert_eq!(scale.size.width, px(PANEL_WIDTH));
        visual.simulate_click(handle.center(), Modifiers::default());
    }
}
