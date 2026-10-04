//! Client-side window decorations: a title bar and border resize grips.
//!
//! macOS frames the window itself. GNOME/Wayland often does not: it ignores the
//! server-decoration protocol, so a window can arrive frameless with no draggable
//! edge. When `window.window_decorations()` reports [`Decorations::Client`],
//! [`window_frame`] draws the chrome and routes dragging and resizing to the
//! window. Otherwise it returns the content unchanged.
//!
//! The frame insets the content by [`INSET`] on each edge the compositor is not
//! already managing. That leaves the border band to the resize grips, which sit
//! behind the content at full window size. Without the inset, a full-width title
//! bar covers the top band and the top edge and top corners cannot be grabbed.

use gpui::{
    AnyElement, Bounds, CursorStyle, Decorations, HitboxBehavior, IntoElement, MouseButton, Pixels,
    Point, ResizeEdge, SharedString, Size, Window, canvas, div, point, prelude::*, px, rgb,
};

/// Width of the border band that acts as resize grips.
const INSET: Pixels = px(6.);
/// Height of the title bar.
const TITLE_BAR_HEIGHT: Pixels = px(32.);

const TITLE_BG: u32 = 0x20242a;
const TITLE_FG: u32 = 0xd8dbe0;
const CONTROL_HOVER: u32 = 0x3a3f47;

/// Wrap `content` in client-side decorations when the platform asks for them.
///
/// The title bar is draggable (left press), opens the window menu on right
/// click, and carries minimize, maximize and close buttons. The border becomes
/// resize grips. On platforms that draw their own frame, or when the window is
/// decorated by the server, `content` is returned unchanged.
pub fn window_frame(
    window: &mut Window,
    title: impl Into<SharedString>,
    content: impl IntoElement,
) -> AnyElement {
    let content = content.into_any_element();
    let Decorations::Client { tiling } = window.window_decorations() else {
        return content;
    };

    window.set_client_inset(INSET);

    let body = div()
        .flex()
        .flex_col()
        .size_full()
        .child(title_bar(title.into()))
        // The content fills the space left below the title bar.
        .child(div().flex_1().min_h_0().child(content));

    div()
        .id("window-frame")
        .size_full()
        // The grip band has a background so it reads as window chrome; at the
        // top it continues the title bar to the window edge.
        .bg(rgb(TITLE_BG))
        .when(!tiling.top, |frame| frame.pt(INSET))
        .when(!tiling.bottom, |frame| frame.pb(INSET))
        .when(!tiling.left, |frame| frame.pl(INSET))
        .when(!tiling.right, |frame| frame.pr(INSET))
        .child(resize_canvas(INSET))
        .on_mouse_move(|_event, window, _cx| window.refresh())
        .on_mouse_down(MouseButton::Left, |event, window, _cx| {
            let size = window.window_bounds().get_bounds().size;
            if let Some(edge) = resize_edge(event.position, INSET, size) {
                window.start_window_resize(edge);
            }
        })
        .child(body)
        .into_any_element()
}

/// The title bar: drag to move, right-click for the window menu, and platform
/// controls on the right.
fn title_bar(title: SharedString) -> impl IntoElement {
    let controls = div()
        .flex()
        .flex_row()
        .items_center()
        .child(control_button("win-min", "\u{2013}", |window| {
            window.minimize_window()
        }))
        .child(control_button("win-max", "\u{25A1}", |window| {
            window.zoom_window()
        }))
        .child(control_button("win-close", "\u{2715}", |window| {
            window.remove_window()
        }));

    div()
        .id("titlebar")
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .h(TITLE_BAR_HEIGHT)
        .px_3()
        .bg(rgb(TITLE_BG))
        .on_mouse_down(MouseButton::Left, |_event, window, _cx| {
            window.start_window_move()
        })
        .on_click(|event, window, _cx| {
            if event.is_right_click() {
                window.show_window_menu(event.position());
            }
        })
        .child(div().text_sm().text_color(rgb(TITLE_FG)).child(title))
        .child(controls)
}

fn control_button(
    id: &str,
    glyph: &str,
    action: impl Fn(&mut Window) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(id.to_string()))
        .w(px(30.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(|style| style.bg(rgb(CONTROL_HOVER)))
        // Keep the title-bar drag from swallowing the click.
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.stop_propagation()
        })
        .on_click(move |_event, window, _cx| action(window))
        .child(glyph.to_string())
}

/// Which edge or corner of the window a point is on, within `inset` pixels of
/// the border. `None` means the point is in the content area.
fn resize_edge(pos: Point<Pixels>, inset: Pixels, size: Size<Pixels>) -> Option<ResizeEdge> {
    let near_left = pos.x < inset;
    let near_right = pos.x > size.width - inset;
    let near_top = pos.y < inset;
    let near_bottom = pos.y > size.height - inset;
    match (near_left, near_right, near_top, near_bottom) {
        (true, _, true, _) => Some(ResizeEdge::TopLeft),
        (_, true, true, _) => Some(ResizeEdge::TopRight),
        (true, _, _, true) => Some(ResizeEdge::BottomLeft),
        (_, true, _, true) => Some(ResizeEdge::BottomRight),
        (_, _, true, _) => Some(ResizeEdge::Top),
        (_, _, _, true) => Some(ResizeEdge::Bottom),
        (true, _, _, _) => Some(ResizeEdge::Left),
        (_, true, _, _) => Some(ResizeEdge::Right),
        _ => None,
    }
}

fn cursor_for(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    }
}

/// An invisible full-window canvas that registers a window-wide hitbox and sets
/// the resize cursor when the pointer is near an edge.
fn resize_canvas(inset: Pixels) -> impl IntoElement {
    canvas(
        move |_bounds, window, _cx| {
            window.insert_hitbox(
                Bounds::new(
                    point(px(0.), px(0.)),
                    window.window_bounds().get_bounds().size,
                ),
                HitboxBehavior::Normal,
            )
        },
        move |_bounds, hitbox, window, _cx| {
            let size = window.window_bounds().get_bounds().size;
            if let Some(edge) = resize_edge(window.mouse_position(), inset, size) {
                window.set_cursor_style(cursor_for(edge), &hitbox);
            }
        },
    )
    .absolute()
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::size;

    /// Pure geometry: the resize-grip hit regions hug the border and leave the
    /// content area alone.
    #[test]
    fn resize_edge_detects_corners_edges_and_content() {
        let inset = px(6.);
        let size = size(px(1000.), px(800.));

        // Corners take precedence over the plain edges they touch.
        assert_eq!(
            resize_edge(point(px(2.), px(2.)), inset, size),
            Some(ResizeEdge::TopLeft)
        );
        assert_eq!(
            resize_edge(point(px(998.), px(2.)), inset, size),
            Some(ResizeEdge::TopRight)
        );
        assert_eq!(
            resize_edge(point(px(2.), px(798.)), inset, size),
            Some(ResizeEdge::BottomLeft)
        );
        assert_eq!(
            resize_edge(point(px(998.), px(798.)), inset, size),
            Some(ResizeEdge::BottomRight)
        );

        // Edges, away from the corners.
        assert_eq!(
            resize_edge(point(px(500.), px(2.)), inset, size),
            Some(ResizeEdge::Top)
        );
        assert_eq!(
            resize_edge(point(px(500.), px(798.)), inset, size),
            Some(ResizeEdge::Bottom)
        );
        assert_eq!(
            resize_edge(point(px(2.), px(400.)), inset, size),
            Some(ResizeEdge::Left)
        );
        assert_eq!(
            resize_edge(point(px(998.), px(400.)), inset, size),
            Some(ResizeEdge::Right)
        );

        // The grip band is the outer `inset` pixels: x < inset is a grip, and
        // the first pixel at or past `inset` is content.
        assert_eq!(
            resize_edge(point(px(5.), px(400.)), inset, size),
            Some(ResizeEdge::Left)
        );
        assert_eq!(resize_edge(point(px(6.), px(400.)), inset, size), None);
        assert_eq!(resize_edge(point(px(500.), px(500.)), inset, size), None);
    }

    /// Every grip has a sensible cursor shape, and the four corners differ from
    /// the four edges (so drags are discoverable).
    #[test]
    fn cursors_match_each_resize_direction() {
        assert_eq!(cursor_for(ResizeEdge::Top), CursorStyle::ResizeUpDown);
        assert_eq!(cursor_for(ResizeEdge::Bottom), CursorStyle::ResizeUpDown);
        assert_eq!(cursor_for(ResizeEdge::Left), CursorStyle::ResizeLeftRight);
        assert_eq!(cursor_for(ResizeEdge::Right), CursorStyle::ResizeLeftRight);
        assert_eq!(
            cursor_for(ResizeEdge::TopLeft),
            CursorStyle::ResizeUpLeftDownRight
        );
        assert_eq!(
            cursor_for(ResizeEdge::BottomRight),
            CursorStyle::ResizeUpLeftDownRight
        );
        assert_eq!(
            cursor_for(ResizeEdge::TopRight),
            CursorStyle::ResizeUpRightDownLeft
        );
        assert_eq!(
            cursor_for(ResizeEdge::BottomLeft),
            CursorStyle::ResizeUpRightDownLeft
        );
    }
}
