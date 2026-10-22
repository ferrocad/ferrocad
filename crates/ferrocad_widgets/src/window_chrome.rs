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
const CONTROL_FG: u32 = 0xc9ced6;
const CONTROL_HOVER: u32 = 0x363c46;
/// The close button gets a red hover, matching native window controls.
const CONTROL_CLOSE_HOVER: u32 = 0xb03a3a;

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
    window_frame_with_leading(window, title, None, content)
}

/// Like [`window_frame`], with an optional element at the **leading** edge of the
/// title bar (before the title).
///
/// The main window puts its app menu there; the Dev Tools window passes no leading
/// element, so its title bar carries only the title and the window controls.
///
/// When the platform draws the decorations (macOS, some Wayland compositors) there is
/// no client title bar to put the element in. Rather than dropping it — which would
/// make an app menu unreachable — the frame draws a slim leading bar directly below
/// the platform title bar. A leading control stays at the same visual level either
/// way; only the frame ownership differs.
pub fn window_frame_with_leading(
    window: &mut Window,
    title: impl Into<SharedString>,
    leading: Option<AnyElement>,
    content: impl IntoElement,
) -> AnyElement {
    let content = content.into_any_element();
    let Decorations::Client { tiling } = window.window_decorations() else {
        let Some(leading) = leading else {
            return content;
        };
        return div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .id("leading-bar")
                    .debug_selector(|| "leading-bar".to_string())
                    .flex()
                    .flex_row()
                    .items_center()
                    .h(TITLE_BAR_HEIGHT)
                    .px_3()
                    .bg(rgb(TITLE_BG))
                    .child(leading),
            )
            .child(div().flex_1().min_h_0().child(content))
            .into_any_element();
    };

    // A maximized/fullscreen window has no draggable edge. Keeping the 6 px inset
    // would shift the content (and therefore the viewport's pointer mapping), and
    // the full-window resize canvas would keep setting a resize cursor over it.
    let maximized = window.is_maximized()
        || matches!(window.window_bounds(), gpui::WindowBounds::Fullscreen(_));
    let inset = frame_inset(maximized);
    window.set_client_inset(inset);

    let body = div()
        .flex()
        .flex_col()
        .size_full()
        .child(title_bar(title.into(), leading))
        // The content fills the space left below the title bar.
        .child(div().flex_1().min_h_0().child(content));

    div()
        .id("window-frame")
        .size_full()
        // The grip band has a background so it reads as window chrome; at the
        // top it continues the title bar to the window edge.
        .bg(rgb(TITLE_BG))
        .when(!maximized && !tiling.top, |frame| frame.pt(inset))
        .when(!maximized && !tiling.bottom, |frame| frame.pb(inset))
        .when(!maximized && !tiling.left, |frame| frame.pl(inset))
        .when(!maximized && !tiling.right, |frame| frame.pr(inset))
        .when(!maximized, |frame| frame.child(resize_canvas(INSET)))
        .on_mouse_move(|_event, window, _cx| window.refresh())
        .on_mouse_down(MouseButton::Left, move |event, window, _cx| {
            if maximized {
                return;
            }
            let size = window.window_bounds().get_bounds().size;
            if let Some(edge) = resize_edge(event.position, INSET, size) {
                window.start_window_resize(edge);
            }
        })
        .child(body)
        .into_any_element()
}

/// The client inset and resize grips collapse on a maximized/fullscreen window.
fn frame_inset(maximized: bool) -> Pixels {
    if maximized {
        px(0.)
    } else {
        INSET
    }
}

/// The title bar: an optional leading element, the title, then the platform
/// controls.
///
/// It drags the window on left press and opens the window menu on right click.
/// A leading control that handles its own clicks must stop mouse-down propagation,
/// or the drag handler would also start a window move (the shell's app menu does).
pub fn title_bar(title: impl Into<SharedString>, leading: Option<AnyElement>) -> impl IntoElement {
    let mut label = div().flex().flex_row().items_center().gap_2();
    if let Some(leading) = leading {
        label = label.child(leading);
    }
    label = label.child(
        div()
            .text_sm()
            .text_color(rgb(TITLE_FG))
            .child(title.into()),
    );

    let controls = div()
        .flex()
        .flex_row()
        .items_center()
        .child(control_button("win-min", "\u{2013}", CONTROL_HOVER, |window| {
            window.minimize_window()
        }))
        .child(control_button("win-max", "\u{25A1}", CONTROL_HOVER, |window| {
            window.zoom_window()
        }))
        .child(control_button(
            "win-close",
            "\u{2715}",
            CONTROL_CLOSE_HOVER,
            |window| window.remove_window(),
        ));

    div()
        .id("titlebar")
        .debug_selector(|| "titlebar".to_string())
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .h(TITLE_BAR_HEIGHT)
        .px_3()
        .bg(rgb(TITLE_BG))
        .text_color(rgb(TITLE_FG))
        .on_mouse_down(MouseButton::Left, |_event, window, _cx| {
            window.start_window_move()
        })
        .on_click(|event, window, _cx| {
            if event.is_right_click() {
                window.show_window_menu(event.position());
            }
        })
        .child(label)
        .child(controls)
}

fn control_button(
    id: &str,
    glyph: &str,
    hover_bg: u32,
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
        .text_color(rgb(CONTROL_FG))
        .hover(move |style| style.bg(rgb(hover_bg)))
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

    /// A maximized window must not reserve the border band or show grips.
    #[test]
    fn the_frame_inset_collapses_when_maximized() {
        assert_eq!(frame_inset(false), INSET);
        assert_eq!(frame_inset(true), px(0.));
    }

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
