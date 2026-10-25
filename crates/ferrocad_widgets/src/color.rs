//! Colour rows for the property editor: a swatch and a small fixed palette.
//!
//! gpui has no colour picker, and a settings value like `#94a3b8` is just a string
//! in a text field. [`swatch`] shows the colour at a glance and [`palette`] is the
//! picker a host expands under the row — deliberately not an overlay, so it works
//! inside the settings card and inside a scrolling Dev Tools pane without an
//! anchoring sidecar.
//!
//! The widget is presentation only: the host owns which row is open and commits
//! the picked `#rrggbb` through its normal property path.

use std::rc::Rc;

use gpui::{
    App, ClickEvent, IntoElement, MouseButton, SharedString, Window, div, prelude::*, px, rgb, rgba,
};

use ferrocad_theme::ui::{CHROME_LINE, HOVER_RIM, PANEL_ALT};

/// The colours the palette offers, packed `0xRRGGBB`: two rows of neutrals and
/// accents that match the visual language.
pub const PALETTE: [u32; 12] = [
    0xffffff, 0xd8dbe0, 0x94a3b8, 0x475569, 0x1d2026, 0x0f1218, 0xf87171, 0xf59e0b, 0x4ade80,
    0x38bdf8, 0xa78bfa, 0xef4444,
];

/// A palette swatch's side, and the colour swatch's side.
const SWATCH: f32 = 18.0;

/// Parse `#rgb`, `#rrggbb` or `#rrggbbaa` (the alpha is ignored) into RGB.
pub fn parse_hex(text: &str) -> Option<[u8; 3]> {
    let hex = text.trim().strip_prefix('#')?;
    if !hex.is_ascii() {
        return None;
    }
    match hex.len() {
        3 => {
            let mut color = [0u8; 3];
            for (index, digit) in hex.bytes().enumerate() {
                color[index] = (digit as char).to_digit(16)? as u8 * 17;
            }
            Some(color)
        }
        6 | 8 => Some([
            u8::from_str_radix(&hex[0..2], 16).ok()?,
            u8::from_str_radix(&hex[2..4], 16).ok()?,
            u8::from_str_radix(&hex[4..6], 16).ok()?,
        ]),
        _ => None,
    }
}

/// The canonical lowercase `#rrggbb` for a colour string, if it parses.
pub fn normalize_hex(text: &str) -> Option<String> {
    parse_hex(text).map(|[r, g, b]| format!("#{r:02x}{g:02x}{b:02x}"))
}

/// A colour swatch button. `open` draws the accent rim while its palette is
/// expanded; `value` fills it (an unparseable value draws an empty swatch).
pub fn swatch(
    key: impl Into<SharedString>,
    value: &str,
    open: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let key = key.into();
    let selector = key.clone();
    let fill = parse_hex(value)
        .map(|[r, g, b]| rgb(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)));
    let mut element = div()
        .id(key)
        .debug_selector(move || format!("{selector}-swatch"))
        .w(px(SWATCH))
        .h(px(SWATCH))
        .flex_shrink_0()
        .rounded_sm()
        .border_1()
        .border_color(rgb(if open { HOVER_RIM } else { CHROME_LINE }))
        .cursor_pointer()
        .hover(|style| style.border_color(rgb(HOVER_RIM)))
        .on_click(on_click);
    element = match fill {
        Some(fill) => element.bg(fill),
        None => element.bg(rgb(PANEL_ALT)),
    };
    element
}

/// The palette a host expands under a colour row.
///
/// `current` gets an accent rim so the stored value is visible in the grid;
/// `on_pick` receives the clicked colour as `#rrggbb`.
pub fn palette(
    key: impl Into<SharedString>,
    current: &str,
    on_pick: impl Fn(&str, &ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let key = key.into();
    let on_pick: Rc<dyn Fn(&str, &ClickEvent, &mut Window, &mut App)> = Rc::new(on_pick);
    let current = normalize_hex(current);
    let selector = key.clone();

    let mut grid = div()
        .id(SharedString::from(format!("{key}-palette")))
        .debug_selector(move || format!("{selector}-palette"))
        .flex()
        .flex_row()
        .flex_wrap()
        .gap_1()
        .p_1()
        .rounded_sm()
        .bg(rgb(PANEL_ALT))
        .border_1()
        .border_color(rgba(CHROME_LINE))
        // A press in the palette must not reach the canvas or its host row.
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.stop_propagation()
        });

    for color in PALETTE {
        let hex = format!("#{color:06x}");
        let selected = current.as_deref() == Some(hex.as_str());
        let cell = format!("{key}-pick-{color:06x}");
        let cell_selector = cell.clone();
        let on_pick = on_pick.clone();
        grid = grid.child(
            div()
                .id(SharedString::from(cell))
                .debug_selector(move || cell_selector.clone())
                .w(px(SWATCH))
                .h(px(SWATCH))
                .rounded_sm()
                .bg(rgb(color))
                .border_1()
                .border_color(rgb(if selected { HOVER_RIM } else { CHROME_LINE }))
                .cursor_pointer()
                .hover(|style| style.border_color(rgb(HOVER_RIM)))
                .on_click(move |event, window, cx| on_pick(&hex, event, window, cx)),
        );
    }
    grid
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui::{Context, Modifiers, Render, TestAppContext, VisualTestContext, Window, px};

    use super::*;

    #[test]
    fn hex_parsing_accepts_the_three_forms() {
        assert_eq!(parse_hex("#94a3b8"), Some([0x94, 0xa3, 0xb8]));
        assert_eq!(parse_hex("#fff"), Some([0xff, 0xff, 0xff]));
        assert_eq!(parse_hex("#94a3b8cc"), Some([0x94, 0xa3, 0xb8]));
        assert_eq!(parse_hex(" #94A3B8 "), Some([0x94, 0xa3, 0xb8]));
    }

    #[test]
    fn hex_parsing_rejects_junk() {
        for text in ["", "#", "94a3b8", "#94a3b", "#gggggg", "#94a3b8c", "#üüü"] {
            assert_eq!(parse_hex(text), None, "{text:?} is not a colour");
        }
    }

    #[test]
    fn normalizing_produces_lowercase_six_digits() {
        assert_eq!(normalize_hex("#FFF").as_deref(), Some("#ffffff"));
        assert_eq!(normalize_hex("#38BDF8FF").as_deref(), Some("#38bdf8"));
        assert_eq!(normalize_hex("nope"), None);
    }

    struct Harness {
        picked: Rc<RefCell<Vec<String>>>,
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let picked = self.picked.clone();
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(swatch(
                    "color-test",
                    "#38bdf8",
                    true,
                    |_event, _window, _cx| {},
                ))
                .child(palette(
                    "color-test",
                    "#38bdf8",
                    move |hex, _event, _window, _cx| picked.borrow_mut().push(hex.to_string()),
                ))
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    #[gpui::test]
    fn the_swatch_and_palette_render(cx: &mut TestAppContext) {
        let harness = Harness {
            picked: Rc::default(),
        };
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), move |_window, cx| cx.new(|_| harness))
                .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        draw(&mut visual);

        let swatch = visual
            .debug_bounds("color-test-swatch")
            .expect("the swatch is rendered");
        assert_eq!(swatch.size.width, px(SWATCH));
        assert!(
            visual.debug_bounds("color-test-palette").is_some(),
            "the palette is rendered"
        );
        assert!(
            visual.debug_bounds("color-test-pick-38bdf8").is_some(),
            "the current colour is in the palette"
        );
    }

    #[gpui::test]
    fn clicking_a_preset_reports_its_hex(cx: &mut TestAppContext) {
        let harness = Harness {
            picked: Rc::default(),
        };
        let picked = harness.picked.clone();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), move |_window, cx| cx.new(|_| harness))
                .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        draw(&mut visual);

        let bounds = visual
            .debug_bounds("color-test-pick-ef4444")
            .expect("the red preset is rendered");
        visual.simulate_click(bounds.center(), Modifiers::default());
        assert_eq!(&*picked.borrow(), &["#ef4444".to_string()]);
    }
}
