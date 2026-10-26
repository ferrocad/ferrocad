//! Form controls: a toggle switch and a select box.
//!
//! The property editor and the command dialog both render `bool` and `enum`
//! fields. A text field that wants `true`/`false`, and an enum you must spell
//! exactly, do not belong in the dielectric language, so these two widgets replace
//! them: [`toggle`] is the boolean switch, [`select_box`] is the closed enum, and
//! [`choice_list`] is the dropdown body the host anchors under it.
//!
//! All three are presentation only. The host owns the value and commits the click;
//! a `bool` click means "flip it", an enum pick arrives as the chosen label.

use std::rc::Rc;

use gpui::{
    App, ClickEvent, IntoElement, MouseButton, SharedString, Window, div, prelude::*, px, rgb, rgba,
};

use ferrocad_theme::ui::{ACTIVE_ACCENT, CHROME_LINE, FG, MUTED, PANEL_ALT, ROW_HOVER};

/// A toggle switch's track.
const TRACK_W: f32 = 34.0;
const TRACK_H: f32 = 18.0;
/// The knob's diameter; the track padding is derived so it stays centred.
const KNOB: f32 = 14.0;

/// A pill switch: the accent track and a knob at the far end when `checked`.
pub fn toggle(
    key: impl Into<SharedString>,
    checked: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let key = key.into();
    let selector = key.clone();
    div()
        .id(key)
        .debug_selector(move || format!("{selector}-toggle"))
        .flex()
        .flex_row()
        .items_center()
        .w(px(TRACK_W))
        .h(px(TRACK_H))
        .px(px((TRACK_H - KNOB) / 2.0))
        .flex_shrink_0()
        .rounded_full()
        .bg(rgb(if checked { ACTIVE_ACCENT } else { PANEL_ALT }))
        .border_1()
        .border_color(rgba(CHROME_LINE))
        .cursor_pointer()
        .when(checked, |element| element.justify_end())
        .child(
            div()
                .w(px(KNOB))
                .h(px(KNOB))
                .rounded_full()
                .bg(rgb(if checked { FG } else { MUTED })),
        )
        .on_click(on_click)
}

/// A select box: the current value and a caret. Clicking it opens the host's
/// dropdown; `open` draws the accent rim while that dropdown is up.
pub fn select_box(
    key: impl Into<SharedString>,
    value: &str,
    open: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let key = key.into();
    let selector = key.clone();
    div()
        .id(key)
        .debug_selector(move || format!("{selector}-select"))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap_1()
        .w_full()
        .min_w_0()
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(rgb(PANEL_ALT))
        .border_1()
        .border_color(rgb(if open { ACTIVE_ACCENT } else { CHROME_LINE }))
        .text_xs()
        .text_color(rgb(FG))
        .cursor_pointer()
        .hover(|style| style.border_color(rgb(ACTIVE_ACCENT)))
        .on_click(on_click)
        .child(div().min_w_0().truncate().child(value.to_string()))
        .child(div().text_color(rgb(MUTED)).child("\u{25be}"))
}

/// The dropdown body: one row per choice, the current one accented.
///
/// `on_pick` receives the chosen label. The rows are `{key}-choice-{index}`, so a
/// caller does not need to build a selector from a label.
pub fn choice_list(
    key: impl Into<SharedString>,
    choices: &[String],
    current: &str,
    on_pick: impl Fn(&str, &ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let key = key.into();
    let on_pick: Rc<dyn Fn(&str, &ClickEvent, &mut Window, &mut App)> = Rc::new(on_pick);
    let selector = key.clone();

    let mut list = div()
        .id(SharedString::from(format!("{key}-choices")))
        .debug_selector(move || format!("{selector}-choices"))
        .flex()
        .flex_col()
        .gap_1()
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.stop_propagation()
        });

    for (index, choice) in choices.iter().enumerate() {
        let chosen = choice == current;
        let row_id = format!("{key}-choice-{index}");
        let row_selector = row_id.clone();
        let on_pick = on_pick.clone();
        let label = choice.clone();
        list = list.child(
            div()
                .id(SharedString::from(row_id))
                .debug_selector(move || row_selector.clone())
                .flex()
                .flex_row()
                .items_center()
                .px_2()
                .py_1()
                .rounded_sm()
                .text_xs()
                .text_color(rgb(if chosen { FG } else { MUTED }))
                .when(chosen, |element| element.bg(rgb(ACTIVE_ACCENT)))
                .cursor_pointer()
                .hover(|style| style.bg(rgba(ROW_HOVER)))
                .child(label.clone())
                .on_click(move |event, window, cx| on_pick(&label, event, window, cx)),
        );
    }
    list
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use gpui::{Context, Modifiers, Render, TestAppContext, VisualTestContext, Window, px};

    use super::*;

    struct Harness {
        toggled: Rc<RefCell<usize>>,
        picked: Rc<RefCell<Vec<String>>>,
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let toggled = self.toggled.clone();
            let picked = self.picked.clone();
            let choices = vec!["fast".to_string(), "safe".to_string()];
            div()
                .flex()
                .flex_col()
                .gap_2()
                .w(px(200.))
                .child(toggle("form-test", true, move |_event, _window, _cx| {
                    *toggled.borrow_mut() += 1;
                }))
                .child(select_box(
                    "form-test",
                    "safe",
                    true,
                    |_event, _window, _cx| {},
                ))
                .child(choice_list(
                    "form-test",
                    &choices,
                    "fast",
                    move |choice, _event, _window, _cx| {
                        picked.borrow_mut().push(choice.to_string())
                    },
                ))
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    fn open(
        cx: &mut TestAppContext,
    ) -> (
        VisualTestContext,
        Rc<RefCell<usize>>,
        Rc<RefCell<Vec<String>>>,
    ) {
        let harness = Harness {
            toggled: Rc::default(),
            picked: Rc::default(),
        };
        let toggled = harness.toggled.clone();
        let picked = harness.picked.clone();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), move |_window, cx| cx.new(|_| harness))
                .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        draw(&mut visual);
        (visual, toggled, picked)
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} has no bounds"));
        cx.simulate_click(bounds.center(), Modifiers::default());
    }

    #[gpui::test]
    fn the_controls_render_their_parts(cx: &mut TestAppContext) {
        let (mut visual, _, _) = open(cx);
        for selector in [
            "form-test-toggle",
            "form-test-select",
            "form-test-choices",
            "form-test-choice-0",
            "form-test-choice-1",
        ] {
            assert!(
                visual.debug_bounds(selector).is_some(),
                "{selector} is rendered"
            );
        }
    }

    #[gpui::test]
    fn the_toggle_reports_a_click(cx: &mut TestAppContext) {
        let (mut visual, toggled, _) = open(cx);
        click(&mut visual, "form-test-toggle");
        assert_eq!(*toggled.borrow(), 1);
    }

    #[gpui::test]
    fn a_choice_reports_its_label(cx: &mut TestAppContext) {
        let (mut visual, _, picked) = open(cx);
        click(&mut visual, "form-test-choice-1");
        assert_eq!(&*picked.borrow(), &["safe".to_string()]);
    }
}
