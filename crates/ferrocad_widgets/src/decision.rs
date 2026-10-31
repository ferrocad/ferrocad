//! The accept/cancel decision pill: one flat segmented strip, `✗ │ ✓` (or words).
//!
//! The modal dialog's footer and the Move form's floating finish pair ask the same
//! question, so they render the same strip. The caller supplies the labels, ids and
//! callbacks; the widget owns only the ink, the divider and the attention rim.
//!
//! The look follows `visual-language.md`: a single `WHEEL_GLASS` body with a
//! `DOCK_BORDER` hairline, no nested button boxes, one `CHROME_LINE` divider, and the
//! segments distinguished by **ink** — `✗` neutral until hover (then the subtract tint),
//! `✓` on `CHROME_ACTIVE_BG` with `CHROME_ACTIVE` ink.

use gpui::{
    App, ClickEvent, MouseButton, MouseDownEvent, SharedString, Window, div, prelude::*, px, rgb,
    rgba,
};

use ferrocad_theme::ui::{
    CHROME_ACTIVE, CHROME_ACTIVE_BG, CHROME_LINE, DOCK_BORDER, HIGHLIGHT_TINT, HOVER_RIM, MUTED,
    SUBTRACT_COLOR, WHEEL_GLASS,
};

/// `SUBTRACT_COLOR` at 14 % (0x24), for the discard segment's hover wash.
const SUBTRACT_TINT: u32 = 0xef444424;

/// Which decision a segment makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecisionRole {
    /// Discard: the neutral `✗` / Cancel segment.
    Cancel,
    /// Accept: the accented `✓` / OK segment.
    Accept,
}

/// One segment of the pill.
pub struct DecisionButton {
    /// The element id and debug selector (`dialog-cancel`, `dialog-accept`, …).
    pub id: SharedString,
    /// The glyph or word the segment shows.
    pub label: SharedString,
    /// Whether this discards or accepts.
    pub role: DecisionRole,
    /// The click handler.
    pub on_click: Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>,
}

impl DecisionButton {
    /// A discard segment.
    pub fn cancel(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            role: DecisionRole::Cancel,
            on_click: Box::new(on_click),
        }
    }

    /// An accept segment.
    pub fn accept(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            role: DecisionRole::Accept,
            on_click: Box::new(on_click),
        }
    }
}

/// How the strip is framed.
#[derive(Default)]
pub struct DecisionOptions {
    /// An outer element id and debug selector (`move-finish`). `None` draws no id.
    pub outer_id: Option<SharedString>,
    /// A right-press handler on the strip (the Move pill opens the object wheel there).
    pub on_right_press: Option<Box<dyn Fn(&MouseDownEvent, &mut Window, &mut App) + 'static>>,
}

/// Render the decision pill: `cancel` then `accept`, separated by one hairline.
pub fn decision_pill(
    cancel: DecisionButton,
    accept: DecisionButton,
    options: DecisionOptions,
) -> impl IntoElement {
    let outer = options
        .outer_id
        .unwrap_or_else(|| SharedString::from("decision-pill"));
    let selector = outer.clone();
    let mut pill = div()
        .flex()
        .flex_row()
        .items_center()
        .h(px(24.0))
        .rounded(px(5.0))
        .overflow_hidden()
        .bg(rgba(WHEEL_GLASS))
        .border_1()
        .border_color(rgba(DOCK_BORDER))
        .occlude()
        .id(outer)
        .debug_selector(move || selector.to_string());
    if let Some(handler) = options.on_right_press {
        pill = pill.on_mouse_down(MouseButton::Right, handler);
    }
    pill.child(segment(cancel))
        .child(div().w(px(1.0)).h(px(14.0)).bg(rgba(CHROME_LINE)))
        .child(segment(accept))
}

/// One segment of the strip.
fn segment(button: DecisionButton) -> impl IntoElement {
    let DecisionButton {
        id,
        label,
        role,
        on_click,
    } = button;
    let selector = id.clone();
    let segment = div()
        .id(id)
        .debug_selector(move || selector.to_string())
        .flex()
        .items_center()
        .h_full()
        .px(px(10.0))
        .text_xs()
        .cursor_pointer()
        .child(label);
    let segment = match role {
        DecisionRole::Cancel => segment.text_color(rgb(MUTED)).hover(|style| {
            style
                .bg(rgba(SUBTRACT_TINT))
                .text_color(rgb(SUBTRACT_COLOR))
        }),
        DecisionRole::Accept => segment
            .bg(rgba(CHROME_ACTIVE_BG))
            .text_color(rgb(CHROME_ACTIVE))
            .hover(|style| style.bg(rgba(HIGHLIGHT_TINT)).text_color(rgb(HOVER_RIM))),
    };
    segment.on_click(on_click)
}
