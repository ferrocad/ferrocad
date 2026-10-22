//! Viewport chrome primitives (P0.6): split buttons, anchored popovers, the floating
//! control pill, the action dock and the Dev Tools command list.
//!
//! The chrome is presentation over the command registry: every control runs an id
//! through [`Shell::run_command`](crate::shell::Shell), and every popover row is a
//! command the registry assigned to that surface. Nothing here names a workbench;
//! the only ids it knows are the three primary controls (grid, axes, workplane) and
//! the synthetic provider rows a Grid/Axis popover starts with.
//!
//! The app menu (`☰`) is app-level chrome, not a viewport control: the shell passes it
//! to `window_frame_with_leading`, so it lives in the main window's title bar. It is
//! defined here because its popover is the same primitive as the header ones.
//!
//! Popovers are **not** modal. They have no backdrop and no `occlude`: a click on the
//! canvas still reaches the viewport, and `on_mouse_down_out` dismisses the popover
//! without swallowing the event. `Esc` closes it from the shell's key path. (The pill
//! they hang from does occlude, so a press on the glass does not start an orbit.)

use gpui::{
    Anchor, ClickEvent, Context, IntoElement, MouseButton, MouseDownEvent, Point, Pixels, Render,
    SharedString, Window, anchored, deferred, div, point, prelude::*, px, rgb, rgba,
};

use crate::commands::{self, CommandInfo, Surface};
use crate::python::{AxisSummary, GridSummary, ProviderFeature};
use crate::shell::{ACCENT, FG, MUTED, PANEL, PANEL_ALT, Shell, ShellModel, panel_header};

/// The three header controls' primary command ids.
pub const GRID: &str = "App_ToggleGrid";
pub const AXIS: &str = "App_ToggleAxisCross";
pub const WORKPLANE: &str = "App_ViewWorkplane";
/// The application menu's first entry (hidden from every other surface).
pub const DEV_TOOLS: &str = "Dev_ToggleTools";

/// The visual-language active ink (`CHROME_ACTIVE`): a checked row's dot/check.
const CHROME_ACTIVE: u32 = 0x67d7ff;
/// The 1 px dielectric hairline used between grouped rows and dock actions.
const CHROME_LINE: u32 = 0xffffff14;

/// One row in an anchored popover.
struct PopoverRow {
    id: String,
    label: String,
    enabled: bool,
    checked: bool,
    accel: String,
    /// Draw a 1 px hairline above this row (a group boundary).
    divider_before: bool,
}

impl PopoverRow {
    fn of(command: &CommandInfo) -> Self {
        Self {
            id: command.id.clone(),
            label: command.label().to_string(),
            enabled: command.enabled,
            checked: command.checked,
            accel: command.accel.clone(),
            divider_before: false,
        }
    }
}

/// The provider-switch rows a Grid/Axis popover starts with (T4).
///
/// One row per registered backend. The id is synthetic
/// (`ferrocad-provider:<feature>:<backend>`), so a click reaches
/// `Shell::run_command` and is intercepted as a switch rather than a registry
/// command. The active backend's row is checked.
fn provider_rows(feature: &str, providers: &[crate::python::GridProvider]) -> Vec<PopoverRow> {
    providers
        .iter()
        .map(|provider| PopoverRow {
            id: format!("ferrocad-provider:{feature}:{}", provider.backend),
            label: if provider.label.is_empty() {
                provider.backend.clone()
            } else {
                provider.label.clone()
            },
            enabled: true,
            checked: provider.active,
            accel: String::new(),
            divider_before: false,
        })
        .collect()
}

/// A read-only row above a popover's command rows (T4).
///
/// It reports a value the control owns (the active grid provider, its spacing)
/// rather than offering an action, so it has no command behind it and never
/// responds to a click.
struct PopoverInfo {
    id: String,
    label: String,
}

/// The Grid popover's provider rows (T4), from the summary the shell cached when
/// the popover opened.
///
/// With no provider registered the summary names none, so one muted row says so
/// instead of showing empty values; a missing summary (bridge failure) omits the
/// rows rather than inventing them.
fn grid_info(summary: Option<&GridSummary>) -> Vec<PopoverInfo> {
    let Some(summary) = summary else {
        return Vec::new();
    };
    let provider = summary
        .label
        .as_deref()
        .or(summary.backend.as_deref())
        .filter(|value| !value.is_empty());
    let Some(provider) = provider else {
        return vec![PopoverInfo {
            id: "no-provider".to_string(),
            label: "No grid provider".to_string(),
        }];
    };
    let mut info = vec![PopoverInfo {
        id: "provider".to_string(),
        label: format!("Provider: {provider}"),
    }];
    if let Some(spacing) = summary.spacing.as_deref().filter(|value| !value.is_empty()) {
        info.push(PopoverInfo {
            id: "spacing".to_string(),
            label: format!("Spacing: {spacing}"),
        });
    }
    info
}

/// The Axis popover's provider and geometry rows, from the summary the shell cached
/// when the popover opened.
///
/// With no axis provider registered one muted row says so; a missing summary (bridge
/// failure) omits the rows rather than inventing them. A provider with no display
/// value for its length/width omits that one row.
fn axis_info(summary: Option<&AxisSummary>) -> Vec<PopoverInfo> {
    let Some(summary) = summary else {
        return Vec::new();
    };
    let provider = summary
        .label
        .as_deref()
        .or(summary.backend.as_deref())
        .filter(|value| !value.is_empty());
    let Some(provider) = provider else {
        return vec![PopoverInfo {
            id: "no-provider".to_string(),
            label: "No axis provider".to_string(),
        }];
    };
    let mut info = vec![PopoverInfo {
        id: "provider".to_string(),
        label: format!("Provider: {provider}"),
    }];
    if let Some(length) = summary.length.as_deref().filter(|value| !value.is_empty()) {
        info.push(PopoverInfo {
            id: "length".to_string(),
            label: format!("Length: {length}"),
        });
    }
    if let Some(width) = summary.width.as_deref().filter(|value| !value.is_empty()) {
        info.push(PopoverInfo {
            id: "width".to_string(),
            label: format!("Width: {width}"),
        });
    }
    info
}

/// The viewport environment controls as a compact floating pill: the
/// grid/axes/workplane controls in one row.
///
/// It is not a pane header. The shell overlays it on the canvas' top-left corner, so the
/// scene reaches the title bar instead of losing a full-width row (and the redundant
/// `3D viewport` sub-header) to chrome. The pill is translucent glass: a dark
/// `#151A24CC` tint with a `#FFFFFF14` hairline, the same visual language as the dock.
///
/// It occludes, so a press on the glass does not start an orbit/drag on the canvas
/// beneath it. Its popovers are `deferred(anchored(..))` overlays with their own
/// hitboxes, so they still receive pointer events.
pub(crate) fn viewport_header(
    model: &ShellModel,
    grid: Option<&GridSummary>,
    axis: Option<&AxisSummary>,
    open: Option<&str>,
    anchor: Option<Point<Pixels>>,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let grid_open = open == Some("grid");
    let axis_open = open == Some("axis");
    let wp_open = open == Some("wp");
    let grid_providers = grid
        .map(|summary| provider_rows("grid", &summary.providers))
        .unwrap_or_default();
    let axis_providers = axis
        .map(|summary| provider_rows("axis", &summary.providers))
        .unwrap_or_default();
    let mut controls = div().flex().flex_row().items_center().gap_1();
    if let Some(command) = commands::find(&model.commands, GRID) {
        controls = controls.child(split_button(
            "grid",
            "Grid",
            GRID,
            Surface::Grid,
            grid_open,
            grid_info(grid),
            grid_providers,
            anchor,
            true,
            command,
            model,
            cx,
        ));
    }
    if let Some(command) = commands::find(&model.commands, AXIS) {
        controls = controls.child(split_button(
            "axis",
            "Axes",
            AXIS,
            Surface::Axis,
            axis_open,
            axis_info(axis),
            axis_providers,
            anchor,
            true,
            command,
            model,
            cx,
        ));
    }
    if let Some(command) = commands::find(&model.commands, WORKPLANE) {
        controls = controls.child(split_button(
            "wp",
            "WP",
            WORKPLANE,
            Surface::Workplane,
            wp_open,
            Vec::new(),
            Vec::new(),
            anchor,
            false,
            command,
            model,
            cx,
        ));
    }

    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .p_1()
        .rounded_md()
        .bg(rgba(0x151a24cc))
        .border_1()
        .border_color(rgba(0xffffff14))
        .text_xs()
        .text_color(rgb(MUTED))
        .occlude()
        .debug_selector(|| "viewport-header".to_string())
        .child(controls)
}

/// The application menu's rows.
///
/// These are app-level commands, not viewport environment controls, so they are not
/// on a surface. Today that is the Dev Tools toggle — it is deliberately hidden from
/// the header/dock, and the window starts closed, so without this menu the only way
/// in is the shortcut. The list is the place for About and Preferences as they land.
fn app_rows(model: &ShellModel, dev_tools_open: bool) -> Vec<PopoverRow> {
    let mut rows = Vec::new();
    if let Some(command) = commands::find(&model.commands, DEV_TOOLS) {
        rows.push(PopoverRow {
            id: command.id.clone(),
            label: command.label().to_string(),
            enabled: command.enabled,
            // The registry cannot know whether a window exists; the shell can.
            checked: dev_tools_open,
            accel: command.accel.clone(),
            divider_before: false,
        });
    }
    rows
}

/// The application menu (`☰`), rendered by the shell at the leading edge of the
/// main window's title bar.
///
/// Clicking opens it; opening is not a toggle for the same reason as the split
/// buttons — the outside-press dismissal runs before the click, so a toggle would
/// immediately reopen. `Esc` or a press elsewhere closes it. The control is absent
/// when there is nothing to offer (a build whose registry has no app commands).
///
/// It sits inside the draggable title bar, so its press must not reach the title
/// bar's move handler; the button (and its popover) stop mouse-down propagation.
pub(crate) fn app_menu(
    open: bool,
    model: &ShellModel,
    dev_tools_open: bool,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let rows = app_rows(model, dev_tools_open);
    if rows.is_empty() && !open {
        return div();
    }

    let mut control = div().flex().flex_row().items_center().rounded_sm().child(
        div()
            .id("app-menu")
            .debug_selector(|| "app-menu".to_string())
            .px_2()
            .py_1()
            .rounded_sm()
            .text_xs()
            .text_color(rgb(FG))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(PANEL)))
            .when(open, |el| el.bg(rgb(PANEL)))
            .child("☰")
            // Keep the title-bar drag from swallowing the click.
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _event, _window, cx| this.open_popover("app", cx))),
    );

    if open {
        control = control.child(
            deferred(
                anchored()
                    .anchor(Anchor::TopLeft)
                    .offset(point(px(0.), px(24.)))
                    .snap_to_window_with_margin(px(8.))
                    .child(popover("app", rows, cx)),
            )
            .with_priority(1),
        );
    }
    control
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
    info: Vec<PopoverInfo>,
    providers: Vec<PopoverRow>,
    anchor: Option<Point<Pixels>>,
    state_dot: bool,
    command: &CommandInfo,
    model: &ShellModel,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let mut rows: Vec<PopoverRow> = commands::popover_commands(&model.commands, surface, primary)
        .into_iter()
        .map(PopoverRow::of)
        .collect();
    // Provider switches lead the menu; a hairline separates them from the command
    // rows so the mode group reads apart from the config actions (T4). The
    // backend-specific settings commands are dev-only and never reach this list.
    if !providers.is_empty() {
        let mut combined = providers;
        if let Some(first) = rows.first_mut() {
            first.divider_before = true;
        }
        combined.append(&mut rows);
        rows = combined;
    }

    let mut control = div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .rounded_sm()
        .bg(rgb(if command.checked { ACCENT } else { PANEL_ALT }))
        .when(open, |el| el.border_1().border_color(rgba(0xffffff33)));

    let enabled = command.enabled;
    let mut label_control = div()
        .id(SharedString::from(format!("{key}-label")))
        .debug_selector(move || format!("{key}-label"))
        .px_2()
        .py_1()
        .text_xs()
        .text_color(rgb(if enabled { FG } else { MUTED }))
        .when(enabled, |el| {
            el.cursor_pointer().hover(|style| style.bg(rgb(PANEL)))
        })
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .when(state_dot, |el| {
                    el.child(
                        div()
                            .debug_selector(move || format!("{key}-state-dot"))
                            .w(px(6.))
                            .h(px(6.))
                            .rounded_full()
                            .bg(rgb(if command.checked { CHROME_ACTIVE } else { MUTED })),
                    )
                })
                .child(label),
        )
        .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _window, cx| {
            this.set_popover_anchor(event.position);
            this.open_popover(key, cx);
        }))
        .on_click(cx.listener(move |this, event: &ClickEvent, _window, cx| {
            if event.is_right_click() {
                this.open_popover(key, cx);
            } else if enabled {
                this.run_command(primary, cx);
            }
        }));
    if state_dot {
        // The dot is the persistent on/off cue; the tooltip states it in words on
        // hover (the report was that hovering gave no indication).
        let tip: SharedString = format!(
            "{}: {}",
            label,
            if command.checked { "on" } else { "off" }
        )
        .into();
        label_control = label_control.tooltip(move |_window, cx| {
            cx.new(|_| CommandTooltip { text: tip.clone() }).into()
        });
    }
    control = control.child(label_control);

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
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, _cx| {
                this.set_popover_anchor(event.position);
            }))
            .on_click(cx.listener(move |this, _event, _window, cx| this.open_popover(key, cx))),
    );

    if open {
        // Anchor from the caret press in window coordinates. An `Anchored` child of
        // the flex header reports a negative layout origin, which used to place the
        // panel above the header; the pointer is the one reliable anchor here. The
        // 18 px offset clears the ~27 px control and leaves a 4 px gap.
        let position = anchor
            .map(|pointer| point(pointer.x, pointer.y + px(18.)))
            .unwrap_or_else(|| point(px(0.), px(28.)));
        control = control.child(
            deferred(
                anchored()
                    .anchor(Anchor::TopLeft)
                    .position(position)
                    .snap_to_window_with_margin(px(8.))
                    .child(popover_with_info(key, info, rows, cx)),
            )
            .with_priority(1),
        );
    }
    control
}

/// An anchored popover: a plain column of command rows, dismissed by an outside
/// press. It has no backdrop, so the canvas beneath still receives pointer events.
fn popover(key: &'static str, rows: Vec<PopoverRow>, cx: &mut Context<Shell>) -> impl IntoElement {
    popover_with_info(key, Vec::new(), rows, cx)
}

/// A popover with read-only value rows above its commands (T4).
fn popover_with_info(
    key: &'static str,
    info: Vec<PopoverInfo>,
    rows: Vec<PopoverRow>,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
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
        // The app menu's popover is drawn as a child of the title bar; a press on a
        // row must not also start a window move.
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| cx.stop_propagation())
        .on_mouse_down_out(cx.listener(|this, _event, _window, cx| {
            this.close_popover(cx);
        }));

    if info.is_empty() && rows.is_empty() {
        panel = panel.child(
            div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(rgb(MUTED))
                .child("No commands on this surface"),
        );
    }

    let info_present = !info.is_empty();
    for row in info {
        let PopoverInfo { id, label } = row;
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
    if info_present && !rows.is_empty() {
        panel = panel.child(hairline(1));
    }

    for row in rows {
        let PopoverRow {
            id,
            label,
            enabled,
            checked,
            accel,
            divider_before,
        } = row;
        if divider_before {
            panel = panel.child(hairline(2));
        }
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
            .child(label)
            .child(div().flex_1());
        if !accel.is_empty() {
            el = el.child(div().text_color(rgb(MUTED)).child(accel));
        }
        if checked {
            el = el.child(div().text_color(rgb(CHROME_ACTIVE)).child("✓"));
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

/// A 1 px group divider inside a popover or the dock.
fn hairline(id: usize) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("hairline-{id}")))
        .debug_selector(move || format!("hairline-{id}"))
        .h(px(1.))
        .w_full()
        .my_1()
        .bg(rgba(CHROME_LINE))
}

/// A vertical 1 px divider between dock groups.
fn dock_divider(id: usize) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("dock-divider-{id}")))
        .debug_selector(move || format!("dock-divider-{id}"))
        .w(px(1.))
        .h(px(16.))
        .mx_1()
        .bg(rgba(CHROME_LINE))
}

/// The Dev Tools providers panel: one row per provider feature, with a button per
/// backend.
///
/// This replaces the user-facing Mods dialog: `Mods_Settings` remains registered (and
/// dev-only) in the command list, but the direct switch is here, next to the other
/// developer surfaces. The active backend is accented; a click goes through the same
/// `Shell::activate_provider` path as a header popover row.
pub(crate) fn providers_panel(
    features: &[ProviderFeature],
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let mut panel = div()
        .id("dev-providers")
        .debug_selector(|| "dev-providers".to_string())
        .flex()
        .flex_col()
        .flex_shrink_0()
        .max_h(px(180.))
        .overflow_y_scroll()
        .bg(rgb(PANEL))
        .border_b_1()
        .border_color(rgba(0xffffff14))
        .child(panel_header("Providers"));

    if features.is_empty() {
        return panel.child(
            div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(rgb(MUTED))
                .child("No providers registered"),
        );
    }

    for feature in features {
        let name = feature.feature.replace('_', " ");
        let mut row = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .child(
                div()
                    .w(px(64.))
                    .text_xs()
                    .text_color(rgb(MUTED))
                    .child(name),
            );
        for provider in &feature.providers {
            let feature_id = feature.feature.clone();
            let backend = provider.backend.clone();
            let label = if provider.label.is_empty() {
                backend.clone()
            } else {
                provider.label.clone()
            };
            let active = provider.active;
            row = row.child(
                div()
                    .id(SharedString::from(format!(
                        "dev-provider-{feature_id}-{backend}"
                    )))
                    .debug_selector({
                        let feature_id = feature_id.clone();
                        let backend = backend.clone();
                        move || format!("dev-provider-{feature_id}-{backend}")
                    })
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .text_xs()
                    .text_color(rgb(if active { FG } else { MUTED }))
                    .when(active, |el| el.bg(rgb(ACCENT)))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(PANEL_ALT)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.activate_provider(&feature_id, &backend, cx);
                    }))
                    .child(label),
            );
        }
        panel = panel.child(row);
    }
    panel
}

/// The settings command the gizmo provider ships (dev-only in the registry; the
/// dock builds its row by hand because the control, not the command list, owns it).
const GIZMO_SETTINGS: &str = "GizmoOverlay_Settings";

/// The contextual `Gizmo ▾` control in the action dock.
///
/// The transform gizmo is a **transient tool handle**, not a persistent scene helper:
/// it exists only while a selection arms it. So its look and its settings do not live in
/// the viewport header with `Grid`/`Axes`/`WP` (which can be toggled in an empty scene);
/// the control appears beside the modification actions only when a gizmo is armed, and
/// disappears with the selection. The caret opens upward into the canvas, away from the
/// bottom edge it sits on.
fn gizmo_control(
    has_settings: bool,
    rows: Vec<PopoverRow>,
    open: bool,
    anchor: Option<Point<Pixels>>,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let mut control = div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .rounded_sm()
        .bg(rgb(PANEL_ALT))
        .when(open, |el| el.border_1().border_color(rgba(0xffffff33)));

    control = control.child(
        div()
            .id("dock-gizmo-label")
            .debug_selector(|| "dock-gizmo-label".to_string())
            .px_2()
            .py_1()
            .text_xs()
            .text_color(rgb(FG))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(PANEL)))
            .child("Gizmo")
            .on_click(cx.listener(move |this, _event, _window, cx| {
                // The label edits, the caret switches: the same split as the header
                // controls. Without the provider there is nothing to edit, so fall
                // back to the menu (it still explains the state).
                if has_settings {
                    this.run_command(GIZMO_SETTINGS, cx);
                } else {
                    this.open_popover("gizmo", cx);
                }
            })),
    );

    control = control.child(
        div()
            .id("dock-gizmo-caret")
            .debug_selector(|| "dock-gizmo-caret".to_string())
            .px_1()
            .py_1()
            .text_xs()
            .text_color(rgb(MUTED))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(PANEL)))
            .child("\u{25b4}")
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _window, _cx| {
                this.set_popover_anchor(event.position);
            }))
            .on_click(cx.listener(move |this, _event, _window, cx| this.open_popover("gizmo", cx))),
    );

    if open {
        // The dock hugs the bottom edge, so the menu grows up from the caret. The
        // pointer is the anchor (a flex child reports a negative layout origin here,
        // as in the header).
        let position = anchor
            .map(|pointer| point(pointer.x, pointer.y - px(6.)))
            .unwrap_or_else(|| point(px(0.), px(0.)));
        control = control.child(
            deferred(
                anchored()
                    .anchor(Anchor::BottomLeft)
                    .position(position)
                    .snap_to_window_with_margin(px(8.))
                    .child(popover("gizmo", rows, cx)),
            )
            .with_priority(1),
        );
    }
    control
}

/// The action dock: the `dock` surface as one centred strip over the bottom of the
/// canvas.
///
/// Rows come straight from the registry, so `IsActive` (driven by the selection) is
/// what enables or greys them. Disabled commands stay visible: the dock shows what
/// the workbench can do, not only what is possible right now.
pub(crate) fn action_dock(
    model: &ShellModel,
    provider_features: &[crate::python::ProviderFeature],
    open_popover: Option<&str>,
    anchor: Option<Point<Pixels>>,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let dock = model.surfaces().dock;
    let mut panel = div()
        .id("action-dock")
        .debug_selector(|| "action-dock".to_string())
        // Only the pill owns the hitbox; the wrapper stays transparent so clicks
        // beside it still reach the canvas.
        .occlude()
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

    // The gizmo is contextual: the control exists only while a selection has armed
    // one, so it can never be clicked out of context. It leads the dock because it
    // describes the handle, not a document action.
    if model.gizmo_armed {
        let mut rows = provider_features
            .iter()
            .find(|feature| feature.feature == "gizmo")
            .map(|feature| provider_rows("gizmo", &feature.providers))
            .unwrap_or_default();
        let has_settings = commands::find(&model.commands, GIZMO_SETTINGS).is_some();
        if has_settings {
            rows.push(PopoverRow {
                id: GIZMO_SETTINGS.to_string(),
                label: "Gizmo settings\u{2026}".to_string(),
                enabled: true,
                checked: false,
                accel: String::new(),
                divider_before: !rows.is_empty(),
            });
        }
        panel = panel.child(gizmo_control(
            has_settings,
            rows,
            open_popover == Some("gizmo"),
            anchor,
            cx,
        ));
        panel = panel.child(dock_divider(9));
    }

    let mut last_group: Option<u8> = None;
    for command in dock {
        // Modification (Delete/Move) and creation (workbench actions) read as two
        // groups; the hairline makes the dielectric split visible.
        let group = if matches!(command.id.as_str(), "App_Delete" | "App_Move") {
            0
        } else {
            1
        };
        if last_group.is_some_and(|previous| previous != group) {
            panel = panel.child(dock_divider(group as usize));
        }
        last_group = Some(group);

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

    if last_group.is_some() {
        panel = panel.child(dock_divider(2));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_summary_omits_the_grid_info_rows() {
        assert!(grid_info(None).is_empty());
    }

    #[test]
    fn a_summary_without_a_provider_says_so() {
        let summary = GridSummary {
            feature: "grid".to_string(),
            ..Default::default()
        };
        let info = grid_info(Some(&summary));
        assert_eq!(info.len(), 1);
        assert_eq!(info[0].id, "no-provider");
        assert_eq!(info[0].label, "No grid provider");
    }

    #[test]
    fn the_info_rows_name_the_active_provider_and_spacing() {
        let summary = GridSummary {
            feature: "grid".to_string(),
            backend: Some("ferrocad".to_string()),
            label: Some("FerroCAD (native)".to_string()),
            spacing: Some("10 mm".to_string()),
            providers: vec![
                crate::python::GridProvider {
                    backend: "ferrocad".to_string(),
                    label: "FerroCAD (native)".to_string(),
                    active: true,
                },
                crate::python::GridProvider {
                    backend: "coin".to_string(),
                    label: "Coin (WGSL)".to_string(),
                    active: false,
                },
            ],
        };
        let info = grid_info(Some(&summary));
        let ids: Vec<&str> = info.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, ["provider", "spacing"]);
        assert_eq!(info[0].label, "Provider: FerroCAD (native)");
        assert_eq!(info[1].label, "Spacing: 10 mm");
    }

    #[test]
    fn a_missing_label_falls_back_to_the_backend_and_spacing_is_optional() {
        let summary = GridSummary {
            backend: Some("coin".to_string()),
            ..Default::default()
        };
        let info = grid_info(Some(&summary));
        assert_eq!(info.len(), 1, "no spacing row without a value");
        assert_eq!(info[0].id, "provider");
        assert_eq!(info[0].label, "Provider: coin");
    }

    #[test]
    fn no_axis_summary_omits_the_axis_info_rows() {
        assert!(axis_info(None).is_empty());
    }

    #[test]
    fn an_axis_summary_without_a_provider_says_so() {
        let summary = AxisSummary {
            feature: "axis".to_string(),
            ..Default::default()
        };
        let info = axis_info(Some(&summary));
        assert_eq!(info.len(), 1);
        assert_eq!(info[0].id, "no-provider");
        assert_eq!(info[0].label, "No axis provider");
    }

    #[test]
    fn the_axis_info_rows_name_the_provider_the_length_and_the_width() {
        let summary = AxisSummary {
            feature: "axis".to_string(),
            backend: Some("ferrocad".to_string()),
            label: Some("FerroCAD (native)".to_string()),
            length: Some("80 px".to_string()),
            width: Some("1.5 px".to_string()),
            providers: vec![
                crate::python::GridProvider {
                    backend: "ferrocad".to_string(),
                    label: "FerroCAD (native)".to_string(),
                    active: true,
                },
                crate::python::GridProvider {
                    backend: "coin".to_string(),
                    label: "Coin (WGSL)".to_string(),
                    active: false,
                },
            ],
        };
        let info = axis_info(Some(&summary));
        let ids: Vec<&str> = info.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(ids, ["provider", "length", "width"]);
        assert_eq!(info[0].label, "Provider: FerroCAD (native)");
        assert_eq!(info[1].label, "Length: 80 px");
        assert_eq!(info[2].label, "Width: 1.5 px");
    }

    #[test]
    fn a_missing_axis_label_falls_back_to_the_backend_and_the_values_are_optional() {
        let summary = AxisSummary {
            backend: Some("coin".to_string()),
            ..Default::default()
        };
        let info = axis_info(Some(&summary));
        assert_eq!(info.len(), 1, "no length/width rows without values");
        assert_eq!(info[0].id, "provider");
        assert_eq!(info[0].label, "Provider: coin");
    }

    #[test]
    fn provider_rows_name_the_switch_id_and_the_active_backend() {
        let providers = vec![
            crate::python::GridProvider {
                backend: "ferrocad".to_string(),
                label: "FerroCAD (native)".to_string(),
                active: true,
            },
            crate::python::GridProvider {
                backend: "coin".to_string(),
                label: String::new(),
                active: false,
            },
        ];
        let rows = provider_rows("grid", &providers);
        assert_eq!(rows.len(), 2, "one row per registered backend");
        assert_eq!(rows[0].id, "ferrocad-provider:grid:ferrocad");
        assert_eq!(rows[0].label, "FerroCAD (native)");
        assert!(rows[0].enabled);
        assert!(rows[0].checked, "the active backend is checked");
        assert!(rows[0].accel.is_empty());
        assert_eq!(rows[1].id, "ferrocad-provider:grid:coin");
        assert_eq!(
            rows[1].label, "coin",
            "an empty label falls back to the backend id"
        );
        assert!(!rows[1].checked);
    }

    /// With no provider registered there are no switch rows to prepend.
    #[test]
    fn no_providers_means_no_provider_rows() {
        assert!(provider_rows("axis", &[]).is_empty());
    }

    /// The `Axes` caret lists the two backend settings rows and the reload command;
    /// they are the axis surface's children, so no extra grouping code is needed.
    #[test]
    fn the_axis_popover_lists_both_backends_and_the_reload_command() {
        let list = vec![
            CommandInfo {
                id: AXIS.to_string(),
                surface: "axis".to_string(),
                ..Default::default()
            },
            CommandInfo {
                id: "AxisOverlay_Settings".to_string(),
                surface: "axis".to_string(),
                parent: AXIS.to_string(),
                ..Default::default()
            },
            CommandInfo {
                id: "AxisOverlayCoin_Settings".to_string(),
                surface: "axis".to_string(),
                parent: AXIS.to_string(),
                ..Default::default()
            },
            CommandInfo {
                id: "App_ReloadAxisSettings".to_string(),
                surface: "axis".to_string(),
                parent: AXIS.to_string(),
                ..Default::default()
            },
            CommandInfo {
                id: "App_ToggleGrid".to_string(),
                surface: "grid".to_string(),
                ..Default::default()
            },
        ];
        let ids: Vec<&str> = commands::popover_commands(&list, Surface::Axis, AXIS)
            .into_iter()
            .map(|command| command.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "AxisOverlay_Settings",
                "AxisOverlayCoin_Settings",
                "App_ReloadAxisSettings"
            ]
        );
    }
}
