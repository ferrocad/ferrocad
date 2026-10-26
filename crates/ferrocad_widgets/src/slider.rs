//! A range slider for a bounded number.
//!
//! Settings that declare both `min` and `max` (the gizmo's handle size, a stroke
//! width, an opacity, a snap angle) are unpleasant to set by typing: the useful
//! values are not evenly spaced. [`fraction`] and [`value_at`] therefore use a
//! **log scale when the range starts above zero**, so dragging halfway moves the
//! value by the same *factor* rather than the same amount. A range that includes
//! zero (`0..1` opacity) has no log scale and stays linear.
//!
//! [`trigger`] is the tiny control in the property row that opens the host's
//! slider overlay; this module owns the geometry and the value mapping, the host
//! owns the overlay and the commit.

use gpui::{
    App, ClickEvent, IntoElement, SharedString, Window, canvas, div, fill, point, prelude::*, px,
    rgb, size,
};

use ferrocad_theme::ui::{HOVER_RIM, PANEL_ALT};

/// Whether `min..max` should be dragged on a log scale.
pub fn use_log_scale(min: f64) -> bool {
    min > 0.0
}

/// The 0..1 position of `value` in `min..=max`.
pub fn fraction(value: f64, min: f64, max: f64) -> f32 {
    if !(max > min) {
        return 0.0;
    }
    let position = if use_log_scale(min) {
        (value.max(min).ln() - min.ln()) / (max.ln() - min.ln())
    } else {
        (value - min) / (max - min)
    };
    position.clamp(0.0, 1.0) as f32
}

/// The value at a 0..1 position: the inverse of [`fraction`].
pub fn value_at(position: f32, min: f64, max: f64) -> f64 {
    if !(max > min) {
        return min;
    }
    let position = f64::from(position.clamp(0.0, 1.0));
    if use_log_scale(min) {
        min * (max / min).powf(position)
    } else {
        min + (max - min) * position
    }
}

/// A compact editor string: at most three decimals, no trailing zeros, no `-0`.
pub fn format_value(value: f64) -> String {
    let mut text = format!("{value:.3}");
    if text.contains('.') {
        text = text.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    if text == "-0" {
        text = "0".to_string();
    }
    text
}

/// The tiny control in a property row that opens the slider overlay: a miniature
/// track with the current position marked.
pub fn trigger(
    key: impl Into<SharedString>,
    fraction: f32,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let key = key.into();
    let selector = key.clone();
    let thumb = fraction.clamp(0.0, 1.0);
    let marker = canvas(
        move |_bounds, _window, _cx| {},
        move |bounds, (), window, _cx| {
            let height = px(4.);
            let track = gpui::Bounds::new(
                point(bounds.left(), bounds.center().y - height / 2.0),
                size(bounds.size.width, height),
            );
            window.paint_quad(fill(track, rgb(PANEL_ALT)).corner_radii(px(2.)));
            // A 4 px accent bar marks where the value sits.
            let width = bounds.size.width.0.max(1.0);
            let x = bounds.left().0 + (thumb as f32) * (width - 4.0);
            let marker =
                gpui::Bounds::new(point(px(x), bounds.top()), size(px(4.), bounds.size.height));
            window.paint_quad(fill(marker, rgb(HOVER_RIM)).corner_radii(px(2.)));
        },
    )
    .size_full();

    div()
        .id(key)
        .debug_selector(move || format!("{selector}-trigger"))
        .w(px(22.))
        .h(px(18.))
        .flex_shrink_0()
        .cursor_pointer()
        .child(marker)
        .on_click(on_click)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_positive_range_maps_on_a_log_scale() {
        // 8..400: halfway in log space is the geometric mean, sqrt(3200) ≈ 56.6.
        let middle = value_at(0.5, 8.0, 400.0);
        assert!((middle - 56.568).abs() < 0.01, "middle = {middle}");
        assert!((fraction(56.568, 8.0, 400.0) - 0.5).abs() < 0.001);

        // The ends are exact.
        assert_eq!(value_at(0.0, 8.0, 400.0), 8.0);
        assert!((value_at(1.0, 8.0, 400.0) - 400.0).abs() < 1e-9);
        assert_eq!(fraction(8.0, 8.0, 400.0), 0.0);
        assert_eq!(fraction(400.0, 8.0, 400.0), 1.0);
    }

    #[test]
    fn a_range_starting_at_zero_stays_linear() {
        assert!(!use_log_scale(0.0));
        assert_eq!(value_at(0.5, 0.0, 1.0), 0.5);
        assert_eq!(fraction(0.5, 0.0, 1.0), 0.5);
        assert_eq!(value_at(0.25, 0.0, 180.0), 45.0);
    }

    #[test]
    fn the_mapping_clamps_and_tolerates_a_degenerate_range() {
        assert_eq!(fraction(-5.0, 0.0, 10.0), 0.0);
        assert_eq!(fraction(50.0, 0.0, 10.0), 1.0);
        assert_eq!(value_at(-1.0, 2.0, 4.0), 2.0);
        assert_eq!(value_at(2.0, 2.0, 4.0), 4.0);
        assert_eq!(value_at(0.5, 3.0, 3.0), 3.0, "empty range pins to min");
        assert_eq!(fraction(3.0, 3.0, 3.0), 0.0);
    }

    #[test]
    fn the_editor_string_is_compact() {
        assert_eq!(format_value(80.0), "80");
        assert_eq!(format_value(1.5), "1.5");
        assert_eq!(format_value(0.30000000000000004), "0.3");
        assert_eq!(format_value(-0.0001), "0");
        assert_eq!(format_value(400.0), "400");
    }
}
