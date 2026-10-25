//! A vertical scrollbar for gpui's `Overflow::Scroll` containers.
//!
//! gpui reserves `scrollbar_width` in the layout but paints nothing, and the
//! published `bite-gpui` crates carry no scrollbar element (Zed's lives in its
//! private `ui` crate). A pane with `overflow_y_scroll()` therefore scrolls by
//! wheel with no visual affordance at all.
//!
//! [`scroll_area`] wraps content in a tracked scroll container and overlays a thin
//! thumb; [`scroll_thumb`] is the overlay alone, for views that already own their
//! scroll container (the console's text area).
//!
//! The thumb is painted by a `canvas` callback, so it follows the live scroll
//! offset without the host re-rendering. It can be dragged; a press on the track
//! outside it jumps the scroll to that position.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Bounds, IntoElement, MouseButton, Pixels, ScrollHandle, SharedString, canvas, div, fill, point,
    prelude::*, px, rgb, size,
};

/// The width of the strip that receives scrollbar presses.
pub const TRACK_WIDTH: Pixels = px(10.);
/// The painted thumb's width.
pub const THUMB_WIDTH: Pixels = px(4.);
/// The gap between the thumb and the container's right edge.
pub const THUMB_INSET: Pixels = px(3.);
/// A thumb never gets shorter than this, however long the content is.
pub const MIN_THUMB: Pixels = px(24.);

/// The resting thumb ink: ~20 % white, so it reads on any panel token.
const THUMB_INK: u32 = 0xffffff33;
/// The thumb ink while dragging.
const THUMB_INK_ACTIVE: u32 = 0x38bdf8aa;

/// The thumb height for a viewport of `viewport` and a scroll range of `max`.
fn thumb_height(viewport: f32, max: f32) -> f32 {
    let content = viewport + max;
    if content <= 0.0 {
        return viewport;
    }
    let height = viewport * viewport / content;
    height.max(MIN_THUMB.0).min(viewport)
}

/// The thumb's window-space rect, or `None` when the content fits.
///
/// `offset` is gpui's scroll offset (zero at the top, negative when scrolled down)
/// and `max` is the positive scroll range from [`ScrollHandle::max_offset`].
pub fn thumb_bounds(bounds: Bounds<Pixels>, offset: Pixels, max: Pixels) -> Option<Bounds<Pixels>> {
    let viewport = bounds.size.height.0;
    if max.0 <= 0.0 || viewport <= 0.0 {
        return None;
    }
    let height = thumb_height(viewport, max.0);
    let travel = viewport - height;
    let progress = (-offset.0 / max.0).clamp(0.0, 1.0);
    let top = bounds.top().0 + progress * travel;
    let left = bounds.right().0 - THUMB_INSET.0 - THUMB_WIDTH.0;
    Some(Bounds::new(
        point(px(left), px(top)),
        size(THUMB_WIDTH, px(height)),
    ))
}

/// Where a drag is, between the press and the release.
#[derive(Clone, Copy, Default)]
struct Drag {
    /// The painted track bounds, recorded during paint.
    track: Option<Bounds<Pixels>>,
    /// Pointer y minus the thumb's top when the drag started.
    grab: f32,
    active: bool,
}

/// Move the scroll so the thumb's top follows `pointer_y - grab`.
fn drag_to(scroll: &ScrollHandle, track: Bounds<Pixels>, pointer_y: f32, grab: f32) {
    let max = scroll.max_offset().y.0;
    if max <= 0.0 {
        return;
    }
    let viewport = track.size.height.0;
    let travel = viewport - thumb_height(viewport, max);
    if travel <= 0.0 {
        return;
    }
    let top = pointer_y - track.top().0 - grab;
    let progress = (top / travel).clamp(0.0, 1.0);
    scroll.set_offset(point(px(0.), px(-progress * max)));
}

/// A vertical scrollbar overlay.
///
/// It fills the right edge of the nearest positioned ancestor: give that ancestor
/// `.relative()`, then add this as the last child. It paints a thumb whenever the
/// tracked scroll container overflows, and does nothing when it does not.
pub fn scroll_thumb(key: &'static str, scroll: &ScrollHandle) -> impl IntoElement {
    let drag = Rc::new(Cell::new(Drag::default()));

    let paint_drag = drag.clone();
    let ink_drag = drag.clone();
    let paint_scroll = scroll.clone();
    let thumb = canvas(
        move |bounds, _window, _cx| {
            let mut state = paint_drag.get();
            state.track = Some(bounds);
            paint_drag.set(state);
        },
        move |bounds, (), window, _cx| {
            let Some(rect) =
                thumb_bounds(bounds, paint_scroll.offset().y, paint_scroll.max_offset().y)
            else {
                return;
            };
            let ink = if ink_drag.get().active {
                THUMB_INK_ACTIVE
            } else {
                THUMB_INK
            };
            window.paint_quad(fill(rect, rgb(ink)).corner_radii(px(THUMB_WIDTH.0 / 2.0)));
        },
    )
    .size_full();

    let down_drag = drag.clone();
    let down_scroll = scroll.clone();
    let move_drag = drag.clone();
    let move_scroll = scroll.clone();
    let up_drag = drag.clone();
    let wheel_scroll = scroll.clone();

    div()
        .id(SharedString::from(format!("{key}-thumb")))
        .debug_selector(move || format!("{key}-thumb"))
        .absolute()
        .top(px(0.))
        .bottom(px(0.))
        .right(px(0.))
        .w(TRACK_WIDTH)
        .on_mouse_down(MouseButton::Left, move |event, window, cx| {
            let state = down_drag.get();
            let Some(track) = state.track else {
                return;
            };
            let Some(thumb) =
                thumb_bounds(track, down_scroll.offset().y, down_scroll.max_offset().y)
            else {
                return;
            };
            let y = event.position.y;
            let mut next = state;
            if y >= thumb.top() && y <= thumb.bottom() {
                next.grab = y.0 - thumb.top().0;
            } else {
                // A press on the track jumps the thumb so it centres on the pointer.
                next.grab = thumb.size.height.0 / 2.0;
                drag_to(&down_scroll, track, y.0, next.grab);
            }
            next.active = true;
            down_drag.set(next);
            window.refresh();
            cx.stop_propagation();
        })
        .on_mouse_move(move |event, window, _cx| {
            let state = move_drag.get();
            if !state.active || !event.dragging() {
                return;
            }
            let Some(track) = state.track else {
                return;
            };
            drag_to(&move_scroll, track, event.position.y.0, state.grab);
            window.refresh();
        })
        .on_mouse_up(MouseButton::Left, move |_event, window, _cx| {
            let mut state = up_drag.get();
            state.active = false;
            up_drag.set(state);
            window.refresh();
        })
        // The strip sits above the scroll container, so a wheel event over it would
        // otherwise have no scrollable ancestor. Forward it by hand (no animation,
        // but it keeps the last 10 px of a pane from being a dead zone).
        .on_scroll_wheel(move |event, window, _cx| {
            let delta = event.delta.pixel_delta(px(36.)).y.0;
            let max = wheel_scroll.max_offset().y.0;
            let next = (wheel_scroll.offset().y.0 - delta).clamp(-max, 0.0);
            wheel_scroll.set_offset(point(px(0.), px(next)));
            window.refresh();
        })
        .child(thumb)
}

/// A tracked vertical scroll container with a [`scroll_thumb`] over its right edge.
///
/// The wrapper takes the flex slot the old `overflow_y_scroll()` row occupied and
/// the content grows inside it, so the caller keeps building rows as before.
pub fn scroll_area(
    key: &'static str,
    scroll: &ScrollHandle,
    content: impl IntoElement,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("{key}-area")))
        .debug_selector(move || format!("{key}-area"))
        .relative()
        .flex_1()
        .min_h_0()
        .child(
            div()
                .id(SharedString::from(format!("{key}-scroll")))
                .flex()
                .flex_col()
                .size_full()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(scroll)
                .child(content),
        )
        .child(scroll_thumb(key, scroll))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Bounds<Pixels> {
        Bounds::new(point(px(100.), px(50.)), size(TRACK_WIDTH, px(200.)))
    }

    #[test]
    fn a_fitting_content_paints_no_thumb() {
        assert!(thumb_bounds(track(), px(0.), px(0.)).is_none());
    }

    #[test]
    fn the_thumb_is_sized_by_the_content_ratio() {
        // Content 400 in a 200 viewport: half the track, at the top.
        let top = thumb_bounds(track(), px(0.), px(200.)).expect("a thumb");
        assert_eq!(top.size.height, px(100.));
        assert_eq!(top.top(), px(50.));
        assert_eq!(top.right(), track().right() - THUMB_INSET);
    }

    #[test]
    fn the_thumb_travels_with_the_offset() {
        let middle = thumb_bounds(track(), px(-100.), px(200.)).expect("a thumb");
        assert_eq!(middle.top(), px(100.));

        let bottom = thumb_bounds(track(), px(-200.), px(200.)).expect("a thumb");
        assert_eq!(
            bottom.top(),
            px(150.),
            "the thumb rests at the bottom of the track"
        );

        // The offset is clamped: an overscroll never pushes the thumb out.
        let clamped = thumb_bounds(track(), px(-400.), px(200.)).expect("a thumb");
        assert_eq!(clamped.top(), px(150.));
    }

    #[test]
    fn a_very_long_content_keeps_a_usable_thumb() {
        let tiny = Bounds::new(point(px(0.), px(0.)), size(TRACK_WIDTH, px(40.)));
        let thumb = thumb_bounds(tiny, px(0.), px(4000.)).expect("a thumb");
        assert_eq!(thumb.size.height, MIN_THUMB);
        assert_eq!(
            thumb.bottom(),
            MIN_THUMB,
            "it starts at the top and still fits inside a short viewport"
        );
    }
}
