//! Viewport chrome primitives (P0.6): split buttons, anchored popovers, the header
//! controls, the action dock and the Dev Tools command list.
//!
//! The chrome is presentation over the command registry: every control runs an id
//! through [`Shell::run_command`](crate::shell::Shell), and every popover row is a
//! command the registry assigned to that surface. Nothing here names a workbench;
//! the only ids it knows are the four primary controls (grid, axes, workplane, mods).
//!
//! Popovers are **not** modal. They have no backdrop and no `occlude`: a click on the
//! canvas still reaches the viewport, and `on_mouse_down_out` dismisses the popover
//! without swallowing the event. `Esc` closes it from the shell's key path.

use gpui::{
    Anchor, ClickEvent, Context, IntoElement, MouseButton, Render, SharedString, Window, anchored,
    deferred, div, point, prelude::*, px, rgb, rgba,
};

use crate::commands::{self, CommandInfo, Surface};
use crate::shell::{ACCENT, FG, MUTED, PANEL, PANEL_ALT, Shell, ShellModel, panel_header};

/// The four header controls' primary command ids.
pub const GRID: &str = "App_ToggleGrid";
pub const AXIS: &str = "App_ToggleAxisCross";
pub const WORKPLANE: &str = "App_ViewWorkplane";
pub const MODS: &str = "Mods_Settings";

/// One row in an anchored popover.
struct PopoverRow {
    id: String,
    label: String,
    enabled: bool,
    checked: bool,
}

impl PopoverRow {
    fn of(command: &CommandInfo) -> Self {
        Self {
            id: command.id.clone(),
            label: command.label().to_string(),
            enabled: command.enabled,
            checked: command.checked,
        }
    }
}

/// The viewport pane header: its title, then the grid/axes/workplane/mods controls
/// right-aligned in the same row.
pub(crate) fn viewport_header(
    model: &ShellModel,
    open: Option<&str>,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let grid_open = open == Some("grid");
    let wp_open = open == Some("wp");
    let mods_open = open == Some("mods");
    let mut controls = div().flex().flex_row().items_center().gap_1();
    if let Some(command) = commands::find(&model.commands, GRID) {
        controls = controls.child(split_button(
            "grid",
            "Grid",
            GRID,
            Surface::Grid,
            grid_open,
            command,
            model,
            cx,
        ));
    }
    if let Some(command) = commands::find(&model.commands, AXIS) {
        controls = controls.child(toggle_button("axis", "Axes", AXIS, command, cx));
    }
    if let Some(command) = commands::find(&model.commands, WORKPLANE) {
        controls = controls.child(split_button(
            "wp",
            "WP",
            WORKPLANE,
            Surface::Workplane,
            wp_open,
            command,
            model,
            cx,
        ));
    }
    if let Some(command) = commands::find(&model.commands, MODS) {
        controls = controls.child(split_button(
            "mods",
            "Mods",
            MODS,
            Surface::Mods,
            mods_open,
            command,
            model,
            cx,
        ));
    }

    div()
        .flex()
        .flex_row()
        .items_center()
        .px_2()
        .py_1()
        .bg(rgb(PANEL_ALT))
        .text_xs()
        .text_color(rgb(MUTED))
        .debug_selector(|| "viewport-header".to_string())
        .child(div().flex_1().min_w_0().truncate().child("3D viewport"))
        .child(controls)
}

/// A split button: the label runs `primary`, the caret opens the surface's popover.
///
/// Opening is deliberately not a toggle: an outside click dismisses the popover
/// before the caret's click is dispatched, so a toggle here would immediately reopen
/// it. Clicking the same caret while open keeps it open (use `Esc` or a click
/// elsewhere to dismiss). A right-click on the label opens it too, and never runs the
/// command.
fn split_button(
    key: &'static str,
    label: &'static str,
    primary: &'static str,
    surface: Surface,
    open: bool,
    command: &CommandInfo,
    model: &ShellModel,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let rows: Vec<PopoverRow> = commands::popover_commands(&model.commands, surface, primary)
        .into_iter()
        .map(PopoverRow::of)
        .collect();

    let mut control = div()
        .flex()
        .flex_row()
        .items_center()
        .rounded_sm()
        .bg(rgb(if command.checked { ACCENT } else { PANEL_ALT }))
        .when(open, |el| el.border_1().border_color(rgba(0xffffff33)));

    let enabled = command.enabled;
    control = control.child(
        div()
            .id(SharedString::from(format!("{key}-label")))
            .debug_selector(move || format!("{key}-label"))
            .px_2()
            .py_1()
            .text_xs()
            .text_color(rgb(if enabled { FG } else { MUTED }))
            .when(enabled, |el| {
                el.cursor_pointer().hover(|style| style.bg(rgb(PANEL)))
            })
            .child(label)
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, _event, _window, cx| {
                this.open_popover(key, cx);
            }))
            .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
                if event.is_right_click() {
                    this.open_popover(key, cx);
                } else if enabled {
                    this.run_command(primary, cx);
                }
            })),
    );

    control = control.child(
        div()
            .id(SharedString::from(format!("{key}-caret")))
            .debug_selector(move || format!("{key}-caret"))
            .px_1()
            .py_1()
            .text_xs()
            .text_color(rgb(MUTED))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(PANEL)))
            .child("▾")
            .on_click(cx.listener(move |this, _event, _window, cx| this.open_popover(key, cx))),
    );

    if open {
        control = control.child(
            deferred(
                anchored()
                    .anchor(Anchor::TopRight)
                    .offset(point(px(0.), px(24.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(popover(key, rows, cx)),
            )
            .with_priority(1),
        );
    }
    control
}

/// A plain toggle button (`Axes`): the whole control runs the checkable command.
fn toggle_button(
    key: &'static str,
    label: &'static str,
    primary: &'static str,
    command: &CommandInfo,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("{key}-toggle")))
        .debug_selector(move || format!("{key}-toggle"))
        .px_2()
        .py_1()
        .rounded_sm()
        .text_xs()
        .text_color(rgb(if command.enabled { FG } else { MUTED }))
        .when(command.checked, |el| el.bg(rgb(ACCENT)))
        .when(command.enabled, |el| {
            el.cursor_pointer()
                .hover(|style| style.bg(rgb(PANEL)))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.run_command(primary, cx);
                }))
        })
        .child(label)
}

/// An anchored popover: a plain column of command rows, dismissed by an outside
/// press. It has no backdrop, so the canvas beneath still receives pointer events.
fn popover(key: &'static str, rows: Vec<PopoverRow>, cx: &mut Context<Shell>) -> impl IntoElement {
    let mut panel = div()
        .id(SharedString::from(format!("{key}-popover")))
        .debug_selector(move || format!("{key}-popover"))
        .flex()
        .flex_col()
        .min_w(px(220.))
        .p_1()
        .rounded_md()
        .bg(rgba(0x1d2026f2))
        .border_1()
        .border_color(rgba(0xffffff26))
        .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
            this.close_popover(cx);
        }));

    if rows.is_empty() {
        panel = panel.child(
            div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(rgb(MUTED))
                .child("No commands on this surface"),
        );
    }

    for row in rows {
        let PopoverRow {
            id,
            label,
            enabled,
            checked,
        } = row;
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
            .text_color(rgb(if enabled { FG } else { MUTED }))
            .when(checked, |el| el.bg(rgb(ACCENT)))
            .when(enabled, |el| {
                el.cursor_pointer().hover(|style| style.bg(rgb(PANEL)))
            })
            .child(label);
        if checked {
            el = el.child(div().text_color(rgb(MUTED)).child("✓"));
        }
        if enabled {
            let id = id.clone();
            el = el.on_click(cx.listener(move |this, _event, _window, cx| {
                this.run_command(&id, cx);
            }));
        }
        panel = panel.child(el);
    }
    panel
}

/// The action dock: the `dock` surface as one centred strip over the bottom of the
/// canvas.
///
/// Rows come straight from the registry, so `IsActive` (driven by the selection) is
/// what enables or greys them. Disabled commands stay visible: the dock shows what
/// the workbench can do, not only what is possible right now.
pub(crate) fn action_dock(model: &ShellModel, cx: &mut Context<Shell>) -> impl IntoElement {
    let dock = model.surfaces().dock;
    let mut panel = div()
        .id("action-dock")
        .debug_selector(|| "action-dock".to_string())
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgba(0x16181de6))
        .border_1()
        .border_color(rgba(0xffffff1f));

    for command in dock {
        let id = command.id.clone();
        let enabled = command.enabled;
        let checked = command.checked;
        let mut button = div()
            .id(SharedString::from(format!("dock-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("dock-{id}")
            })
            .px_2()
            .py_1()
            .rounded_sm()
            .text_xs()
            .text_color(rgb(if enabled { FG } else { MUTED }))
            .when(checked, |el| el.bg(rgb(ACCENT)))
            .when(enabled, |el| {
                el.cursor_pointer().hover(|style| style.bg(rgb(PANEL_ALT)))
            })
            .child(command.label().to_string());
        if enabled {
            button = button.on_click(cx.listener(move |this, _event, _window, cx| {
                this.run_command(&id, cx);
            }));
        }
        panel = panel.child(button);
    }

    panel = panel.child(
        div()
            .id("dock-search")
            .debug_selector(|| "dock-search".to_string())
            .px_2()
            .py_1()
            .rounded_sm()
            .text_xs()
            .text_color(rgb(MUTED))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(PANEL)))
            .child("Search (Ctrl+K)")
            .on_click(cx.listener(|this, _event, _window, cx| this.search_stub(cx))),
    );

    div()
        .absolute()
        .left(px(0.))
        .right(px(0.))
        .bottom(px(12.))
        .flex()
        .justify_center()
        .child(panel)
}

/// The Dev Tools command list: **every** registered command, vertically, with its
/// live enablement and the surface that owns it (T6's safety net).
pub(crate) fn command_list_panel(
    model: &ShellModel,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let mut rows = div()
        .id("dev-command-scroll")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll();

    for command in &model.commands {
        let id = command.id.clone();
        let enabled = command.enabled;
        let checked = command.checked;
        let mut row = div()
            .id(SharedString::from(format!("dev-command-{id}")))
            .debug_selector({
                let id = id.clone();
                move || format!("dev-command-{id}")
            })
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .text_xs()
            .text_color(rgb(if enabled { FG } else { MUTED }))
            .when(checked, |el| el.bg(rgb(ACCENT)))
            .when(enabled, |el| {
                el.cursor_pointer().hover(|style| style.bg(rgb(PANEL_ALT)))
            })
            .child(command.label().to_string())
            .child(div().flex_1())
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .child(command.surface().as_str()),
            );
        if !command.tooltip.is_empty() {
            let mut tip = command.tooltip.clone();
            if !command.accel.is_empty() {
                tip.push_str("  ·  ");
                tip.push_str(&command.accel);
            }
            row = row.tooltip(move |_window, cx| {
                cx.new(|_| CommandTooltip {
                    text: tip.clone().into(),
                })
                .into()
            });
        }
        if enabled {
            row = row.on_click(cx.listener(move |this, _event, _window, cx| {
                this.run_command(&id, cx);
            }));
        }
        rows = rows.child(row);
    }

    div()
        .flex()
        .flex_col()
        .h(px(240.))
        .flex_shrink_0()
        .bg(rgb(PANEL))
        .debug_selector(|| "dev-command-list".to_string())
        .child(panel_header("Commands"))
        .child(rows)
}

/// A command row's hover text: `ToolTip` and the accelerator.
struct CommandTooltip {
    text: SharedString,
}

impl Render for CommandTooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_sm()
            .bg(rgb(PANEL_ALT))
            .border_1()
            .border_color(rgba(0xffffff14))
            .text_xs()
            .text_color(rgb(FG))
            .child(self.text.clone())
    }
}
