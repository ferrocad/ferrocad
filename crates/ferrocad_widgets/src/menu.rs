//! A domain-free anchored menu: a column of read-only info rows and clickable
//! command rows, dismissed by a press outside its panel.
//!
//! The widget owns only presentation. The caller supplies the row data and the
//! callbacks ([`menu`]); it keeps the command semantics (which ids are
//! commands, whether a click keeps the menu open, and what a click runs).
//!
//! The panel has no backdrop and is not modal: a press outside it reaches
//! `on_dismiss` without swallowing the event, so a menu can hang over a canvas
//! that still receives the click.

use std::rc::Rc;

use gpui::{
    App, ClickEvent, IntoElement, MouseButton, SharedString, Window, div, prelude::*, px, rgb, rgba,
};

use ferrocad_theme::ui::{
    ACTIVE_ACCENT, CHROME_LINE, DOT_INACTIVE, FG, MUTED, POPOVER_BG, POPOVER_BORDER, ROW_HOVER,
};

/// A read-only value row shown above the commands (for example the active
/// provider or a current value). Info rows are muted and never clickable.
#[derive(Clone, Debug)]
pub struct MenuInfoRow {
    pub id: SharedString,
    pub label: SharedString,
}

/// One clickable command row.
#[derive(Clone, Debug)]
pub struct MenuRow {
    pub id: SharedString,
    pub label: SharedString,
    pub enabled: bool,
    pub checked: bool,
    /// Draw the status-dot slot: a solid dot for an on state and a dim one for
    /// off. A row without this keeps an empty slot so labels stay aligned.
    pub toggle: bool,
    /// An optional shortcut or hint, right-aligned in [`MUTED`].
    pub accel: Option<SharedString>,
    /// Draw a 1 px group hairline above this row.
    pub divider_before: bool,
}

/// Render a menu panel.
///
/// `key` namespaces the debug selectors: `{key}-popover`, `{key}-row-{id}`,
/// `{key}-row-{id}-dot` and `{key}-hairline-{n}`. `on_row` receives the clicked
/// row's id; `on_dismiss` runs on a press outside the panel. The caller decides
/// what a row does and whether the menu stays open.
pub fn menu(
    key: &'static str,
    info: Vec<MenuInfoRow>,
    rows: Vec<MenuRow>,
    on_row: impl Fn(&str, &ClickEvent, &mut Window, &mut App) + 'static,
    on_dismiss: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let on_row: Rc<dyn Fn(&str, &ClickEvent, &mut Window, &mut App)> = Rc::new(on_row);

    let mut panel = div()
        .id(SharedString::from(format!("{key}-popover")))
        .debug_selector(move || format!("{key}-popover"))
        .flex()
        .flex_col()
        .min_w(px(220.))
        .p_1()
        .rounded_md()
        .bg(rgba(POPOVER_BG))
        .border_1()
        .border_color(rgba(POPOVER_BORDER))
        // A press on a row must not also reach the surface the panel hangs
        // from (the title bar or a control pill).
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.stop_propagation()
        })
        .on_mouse_down_out(move |_event, window, cx| on_dismiss(window, cx));

    let empty = info.is_empty() && rows.is_empty();
    let info_present = !info.is_empty();
    let rows_present = !rows.is_empty();
    let mut hairline = 1usize;

    if empty {
        panel = panel.child(
            div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(rgb(MUTED))
                .child("No commands on this surface"),
        );
    }

    for row in info {
        let MenuInfoRow { id, label } = row;
        panel = panel.child(
            div()
                .id(SharedString::from(format!("{key}-row-{id}")))
                .debug_selector({
                    let id = id.clone();
                    move || format!("{key}-row-{id}")
                })
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .rounded_sm()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(label),
        );
    }
    if info_present && rows_present {
        panel = panel.child(hairline_element(key, hairline));
        hairline += 1;
    }

    for row in rows {
        let MenuRow {
            id,
            label,
            enabled,
            checked,
            toggle,
            accel,
            divider_before,
        } = row;

        if divider_before {
            panel = panel.child(hairline_element(key, hairline));
            hairline += 1;
        }

        // The dot and the text carry the checked/off state; the row background
        // is reserved for pointer hover, so hovering always brightens the row
        // under the cursor instead of darkening an active one.
        let text_color = if enabled && (!toggle || checked) {
            FG
        } else {
            MUTED
        };

        let mut el = div()
            .id(SharedString::from(format!("{key}-row-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("{key}-row-{id}")
            })
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .text_xs()
            .text_color(rgb(text_color))
            .when(enabled, |el| {
                el.cursor_pointer()
                    .hover(|style| style.bg(rgba(ROW_HOVER)).text_color(rgb(FG)))
            });

        if toggle {
            el = el.child(
                div()
                    .debug_selector({
                        let id = id.clone();
                        move || format!("{key}-row-{id}-dot")
                    })
                    .w(px(6.))
                    .h(px(6.))
                    .rounded_full()
                    .bg(if checked {
                        rgb(ACTIVE_ACCENT)
                    } else {
                        rgba(DOT_INACTIVE)
                    }),
            );
        } else {
            el = el.child(div().w(px(6.)).h(px(6.)));
        }

        el = el.child(label).child(div().flex_1());
        if let Some(accel) = accel {
            el = el.child(div().text_color(rgb(MUTED)).child(accel));
        }

        if enabled {
            let on_row = on_row.clone();
            let id = id.clone();
            el = el.on_click(
                move |event: &ClickEvent, window: &mut Window, cx: &mut App| {
                    on_row(&id, event, window, cx);
                },
            );
        }

        panel = panel.child(el);
    }

    panel
}

/// A 1 px group divider inside the menu.
fn hairline_element(key: &'static str, index: usize) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("{key}-hairline-{index}")))
        .debug_selector(move || format!("{key}-hairline-{index}"))
        .h(px(1.))
        .w_full()
        .my_1()
        .bg(rgba(CHROME_LINE))
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    use gpui::{Context, Modifiers, Render, TestAppContext, VisualTestContext, Window, point, px};

    use super::*;

    struct Harness {
        info: Vec<MenuInfoRow>,
        rows: Vec<MenuRow>,
        clicked: Rc<RefCell<Vec<String>>>,
        dismissed: Rc<Cell<usize>>,
    }

    impl Render for Harness {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            let clicked = self.clicked.clone();
            let dismissed = self.dismissed.clone();
            menu(
                "menu-test",
                self.info.clone(),
                self.rows.clone(),
                move |id, _event, _window, _cx| clicked.borrow_mut().push(id.to_string()),
                move |_window, _cx| dismissed.set(dismissed.get() + 1),
            )
        }
    }

    fn row(id: &str, toggle: bool) -> MenuRow {
        MenuRow {
            id: id.into(),
            label: format!("Row {id}").into(),
            enabled: true,
            checked: false,
            toggle,
            accel: None,
            divider_before: false,
        }
    }

    fn open(
        cx: &mut TestAppContext,
        info: Vec<MenuInfoRow>,
        rows: Vec<MenuRow>,
    ) -> (VisualTestContext, Rc<RefCell<Vec<String>>>, Rc<Cell<usize>>) {
        let harness = Harness {
            info,
            rows,
            clicked: Rc::default(),
            dismissed: Rc::default(),
        };
        let clicked = harness.clicked.clone();
        let dismissed = harness.dismissed.clone();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), move |_window, cx| cx.new(|_| harness))
                .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        draw(&mut visual);
        (visual, clicked, dismissed)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    fn click(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} has no bounds"));
        cx.simulate_click(bounds.center(), Modifiers::default());
    }

    #[gpui::test]
    fn the_panel_info_and_row_selectors_render(cx: &mut TestAppContext) {
        let (mut visual, _, _) = open(
            cx,
            vec![MenuInfoRow {
                id: "provider".into(),
                label: "Provider: native".into(),
            }],
            vec![row("alpha", false), row("beta", false)],
        );

        for selector in [
            "menu-test-popover",
            "menu-test-row-provider",
            "menu-test-row-alpha",
            "menu-test-row-beta",
        ] {
            assert!(
                visual.debug_bounds(selector).is_some(),
                "{selector} is rendered"
            );
        }
    }

    #[gpui::test]
    fn only_toggle_rows_carry_a_status_dot(cx: &mut TestAppContext) {
        let (mut visual, _, _) = open(
            cx,
            Vec::new(),
            vec![row("toggle", true), row("action", false)],
        );

        assert!(
            visual.debug_bounds("menu-test-row-toggle-dot").is_some(),
            "a toggle row draws its status dot"
        );
        assert!(
            visual.debug_bounds("menu-test-row-action-dot").is_none(),
            "a non-toggle row keeps an empty slot"
        );
    }

    #[gpui::test]
    fn clicking_a_row_reports_its_id(cx: &mut TestAppContext) {
        let (mut visual, clicked, _) = open(cx, Vec::new(), vec![row("alpha", false)]);

        click(&mut visual, "menu-test-row-alpha");

        assert_eq!(clicked.borrow().as_slice(), ["alpha"]);
    }

    #[gpui::test]
    fn clicking_outside_dismisses(cx: &mut TestAppContext) {
        let (mut visual, _, dismissed) = open(cx, Vec::new(), vec![row("alpha", false)]);

        let panel = visual
            .debug_bounds("menu-test-popover")
            .expect("the panel is rendered");
        let outside = point(panel.right() + px(40.), panel.origin.y + px(4.));
        visual.simulate_click(outside, Modifiers::default());

        assert_eq!(dismissed.get(), 1, "a press outside dismisses the menu");
    }

    #[gpui::test]
    fn divider_before_renders_a_hairline(cx: &mut TestAppContext) {
        let mut beta = row("beta", false);
        beta.divider_before = true;
        let (mut visual, _, _) = open(cx, Vec::new(), vec![row("alpha", false), beta]);

        assert!(
            visual.debug_bounds("menu-test-hairline-1").is_some(),
            "the divider renders a hairline above its row"
        );
    }
}
