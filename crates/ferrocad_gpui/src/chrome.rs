//! Viewport chrome primitives (P0.6): split buttons, anchored popovers, the floating
//! control pill, the action dock and the Dev Tools command list.
//!
//! The chrome is presentation over the command registry: every control runs an id
//! through [`Shell::run_command`](crate::shell::Shell), and every popover row is a
//! command the registry assigned to that surface. Nothing here names a workbench;
//! the only ids it knows are the five primary controls (grid, axes, gizmo,
//! workplane, snap) and the synthetic provider rows a Grid/Axis/Gizmo popover
//! starts with.
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
    Anchor, App, ClickEvent, Context, IntoElement, MouseButton, MouseDownEvent, Pixels, Point,
    Render, ScrollHandle, SharedString, Window, anchored, deferred, div, point, prelude::*, px,
    rgb, rgba,
};

use std::collections::{HashMap, HashSet};

use ferrocad_wgpu::{AdapterChoice, AdapterSummary};
use ferrocad_widgets::icon;
use ferrocad_widgets::menu::{MenuInfoRow, MenuRow, menu};
use ferrocad_widgets::scroll::scroll_area;

use ferrocad_theme::ui::{
    ACTIVE_ACCENT, CHROME_ACTIVE, CHROME_LINE, DOCK_BG, DOCK_BORDER, GLASS, ROW_HOVER,
};

use crate::commands::{self, CommandInfo, Surface};
use crate::python::{AxisSummary, GridSummary, ProviderFeature};
use crate::shell::{ACCENT, FG, MUTED, PANEL, PANEL_ALT, Shell, ShellModel};

/// The header controls' primary command ids.
pub const GRID: &str = "App_ToggleGrid";
pub const AXIS: &str = "App_ToggleAxisCross";
/// The gizmo split control's primary command: show or hide the transform gizmo.
pub const GIZMO: &str = "App_ToggleGizmo";
pub const WORKPLANE: &str = "App_ViewWorkplane";
/// The global snap switch. The six `Draft_Snap_*` modes declare this id as their
/// `FerroCAD_Parent`, so they appear in its popover instead of in the dock.
pub const SNAP: &str = "Draft_Snap_Lock";
/// The application menu's first entry (hidden from every other surface).
pub const DEV_TOOLS: &str = "Dev_ToggleTools";

/// The command the Gizmo popover's reset row runs.
const RESET_GIZMO: &str = "App_ResetGizmoAppearance";
/// The provider feature the Gizmo popover switches.
const GIZMO_FEATURE: &str = "gizmo";

// The chrome accents and dielectric lines are in `ferrocad_theme::ui`: the same
// table the popover widget uses, so the pill and its rows cannot drift.

/// One row in an anchored popover.
struct PopoverRow {
    id: String,
    label: String,
    enabled: bool,
    checked: bool,
    /// The row represents a boolean state, so it carries a status dot (a provider
    /// choice and a command toggle both do; a one-shot action does not).
    toggle: bool,
    /// A **state** row (a command toggle or a provider choice) keeps the popover open
    /// after the click, so several options can be set in one visit and the dots
    /// follow. A one-shot action row still dismisses it.
    keep_open: bool,
    accel: String,
    /// Draw a 1 px hairline above this row (a group boundary).
    divider_before: bool,
}

impl PopoverRow {
    fn of(command: &CommandInfo) -> Self {
        Self {
            id: command.id.clone(),
            // A popover sits under its own control, so `Snap Grid` would repeat the
            // control's name six times; the short label keeps the column scannable.
            label: command.short_label().to_string(),
            enabled: command.enabled,
            checked: command.checked,
            toggle: command.checkable,
            keep_open: command.checkable,
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
            // A provider group is a radio set: the dot shows the active backend. It
            // is a **state** row like a checkbox, so the menu stays open and the dot
            // follows; `activate_provider` refreshes the summary in place.
            toggle: true,
            keep_open: true,
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

/// The Gizmo popover's provider row, from the feature the shell read when the
/// popover opened.
///
/// With no gizmo provider registered one muted row says so; a missing feature
/// (bridge failure) omits the rows rather than inventing them, exactly as
/// [`grid_info`] and [`axis_info`] do.
fn gizmo_info(feature: Option<&ProviderFeature>) -> Vec<PopoverInfo> {
    let Some(feature) = feature else {
        return Vec::new();
    };
    let provider = feature
        .providers
        .iter()
        .find(|provider| provider.active)
        .or_else(|| feature.providers.first());
    let Some(provider) = provider else {
        return vec![PopoverInfo {
            id: "no-provider".to_string(),
            label: "No gizmo provider".to_string(),
        }];
    };
    let label = if provider.label.is_empty() {
        provider.backend.clone()
    } else {
        provider.label.clone()
    };
    vec![PopoverInfo {
        id: "provider".to_string(),
        label: format!("Provider: {label}"),
    }]
}

/// The Gizmo popover's rows, in render order.
///
/// Provider choices lead (the mode group), then the `Show gizmo` state row and
/// two actions. `Gizmo settings…` runs the active backend's registered settings
/// command, which selects the `Settings/Gizmo` node — the same path the axis and
/// grid settings rows use, not a second one. The reset row runs the appearance
/// reset command.
fn gizmo_rows(feature: Option<&ProviderFeature>, commands: &[CommandInfo]) -> Vec<PopoverRow> {
    let mut rows = feature
        .map(|feature| provider_rows(GIZMO_FEATURE, &feature.providers))
        .unwrap_or_default();

    // The primary is a state row: the menu stays open so the dot follows.
    if let Some(command) = commands::find(commands, GIZMO) {
        let mut row = PopoverRow::of(command);
        row.label = "Show gizmo".to_string();
        row.divider_before = !rows.is_empty();
        rows.push(row);
    }

    // The active backend owns the settings command; fall back to any backend that
    // advertises one so the row is not lost to a provider without a command.
    let settings = feature.and_then(|feature| {
        feature
            .providers
            .iter()
            .find(|provider| provider.active)
            .and_then(|provider| provider.command.as_deref())
            .or_else(|| {
                feature
                    .providers
                    .iter()
                    .find_map(|provider| provider.command.as_deref())
            })
    });
    if let Some(command) = settings.and_then(|id| commands::find(commands, id)) {
        let mut row = PopoverRow::of(command);
        row.label = "Gizmo settings…".to_string();
        rows.push(row);
    }

    if let Some(command) = commands::find(commands, RESET_GIZMO) {
        rows.push(PopoverRow {
            id: command.id.clone(),
            label: "Reset to edition defaults".to_string(),
            enabled: command.enabled,
            checked: false,
            toggle: false,
            keep_open: false,
            accel: command.accel.clone(),
            divider_before: false,
        });
    }

    rows
}

/// The viewport environment controls as a compact floating pill: the
/// grid/axes/gizmo/workplane controls in one row.
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
    let gizmo_open = open == Some("gizmo");
    let wp_open = open == Some("wp");
    let snap_open = open == Some("snap");
    let grid_providers = grid
        .map(|summary| provider_rows("grid", &summary.providers))
        .unwrap_or_default();
    let axis_providers = axis
        .map(|summary| provider_rows("axis", &summary.providers))
        .unwrap_or_default();
    // The gizmo has no summary bridge and the model does not carry the provider
    // list, so read the registry while the popover is open; a bridge failure
    // leaves the rows out.
    let gizmo_feature = if gizmo_open {
        crate::python::provider_features()
            .ok()
            .and_then(|features| {
                features
                    .into_iter()
                    .find(|feature| feature.feature == GIZMO_FEATURE)
            })
    } else {
        None
    };
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
    // The gizmo is a capability like the grid and axis cross: its label toggles
    // visibility and its caret exposes the provider switch and settings. It sits
    // between WP and Snap so the environment controls keep one visual order.
    if let Some(command) = commands::find(&model.commands, GIZMO) {
        controls = controls.child(split_button_with_rows(
            "gizmo",
            "Gizmo",
            GIZMO,
            gizmo_open,
            gizmo_info(gizmo_feature.as_ref()),
            gizmo_rows(gizmo_feature.as_ref(), &model.commands),
            anchor,
            true,
            command,
            cx,
        ));
    }
    // Snapping is ambient spatial state, so it belongs beside Grid/Axes/WP rather
    // than in the dock next to the selection verbs. The label is the global switch;
    // the six modes live in the popover.
    if let Some(command) = commands::find(&model.commands, SNAP) {
        controls = controls.child(split_button(
            "snap",
            "Snap",
            SNAP,
            Surface::Snap,
            snap_open,
            Vec::new(),
            Vec::new(),
            anchor,
            true,
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
        .bg(rgba(GLASS))
        .border_1()
        .border_color(rgba(CHROME_LINE))
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
            // A toggle, but the app menu is a launcher: clicking closes it so the
            // window it just opened is visible.
            toggle: true,
            keep_open: false,
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
    split_button_with_rows(
        key, label, primary, open, info, rows, anchor, state_dot, command, cx,
    )
}

/// A split button from an explicit row list.
///
/// [`split_button`] derives the rows from the registry surface; the Gizmo control
/// builds its own because its settings commands are dev-only and its provider
/// list is not a summary. The visual language is identical.
fn split_button_with_rows(
    key: &'static str,
    label: &'static str,
    primary: &'static str,
    open: bool,
    info: Vec<PopoverInfo>,
    rows: Vec<PopoverRow>,
    anchor: Option<Point<Pixels>>,
    state_dot: bool,
    command: &CommandInfo,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    // Dielectric: the pill's glass is the surface and the 6 px state dot carries
    // on/off in `ACTIVE_ACCENT` (one ink with the popover rows). A checked control
    // used to fill with `ACCENT`, which pulled the eye away from the model (the
    // NaviCube bar is the reference look).
    let mut control = div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .rounded_sm()
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
                            .bg(rgb(if command.checked { ACTIVE_ACCENT } else { MUTED })),
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
///
/// The panel is the reusable [`ferrocad_widgets::menu`]; this function is the shell
/// adapter: it maps registry commands to rows and turns a click into the command run.
/// A **state** row (`keep_open`) runs in place so the menu stays open and its dot
/// follows; an action row dismisses the menu.
fn popover_with_info(
    key: &'static str,
    info: Vec<PopoverInfo>,
    rows: Vec<PopoverRow>,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let keep_open: HashSet<String> = rows
        .iter()
        .filter(|row| row.keep_open)
        .map(|row| row.id.clone())
        .collect();

    let info = info
        .into_iter()
        .map(|PopoverInfo { id, label }| MenuInfoRow {
            id: id.into(),
            label: label.into(),
        })
        .collect();
    let rows = rows
        .into_iter()
        .map(|row| MenuRow {
            id: row.id.into(),
            label: row.label.into(),
            enabled: row.enabled,
            checked: row.checked,
            toggle: row.toggle,
            accel: (!row.accel.is_empty()).then(|| row.accel.into()),
            divider_before: row.divider_before,
        })
        .collect();

    // The widget is domain-free; route its row and dismissal callbacks back into the
    // shell through a weak handle (no retained cycle through the popover key).
    let shell = cx.entity().downgrade();
    let on_row = move |id: &str, _event: &ClickEvent, _window: &mut Window, cx: &mut App| {
        let id = id.to_string();
        let _ = shell.update(cx, |shell, cx| {
            if keep_open.contains(&id) {
                shell.run_command_in_place(&id, cx);
            } else {
                shell.run_command(&id, cx);
            }
        });
    };
    let shell = cx.entity().downgrade();
    let on_dismiss = move |_window: &mut Window, cx: &mut App| {
        let _ = shell.update(cx, |shell, cx| shell.close_popover(cx));
    };

    menu(key, info, rows, on_row, on_dismiss)
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
    scroll: &ScrollHandle,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let mut rows = div().flex().flex_col();

    if features.is_empty() {
        rows = rows.child(
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
        rows = rows.child(row);
    }

    div()
        .id("dev-providers")
        .debug_selector(|| "dev-providers".to_string())
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .bg(rgb(PANEL))
        .child(scroll_area("dev-providers", scroll, rows))
}

/// The Dev Tools GPU panel (dev-only): the persisted adapter choice and the
/// adapters this machine offers.
///
/// One `Auto` row plus one row per enumerated adapter; the persisted setting is
/// accented and labelled `active`. A click runs a synthetic `ferrocad-gpu:<value>`
/// id through [`Shell::run_command`](crate::shell::Shell), the same interception
/// pattern as the provider rows. The list is enumerated by the shell when the
/// window opens, never per frame.
///
/// The forced adapter belongs to the **image path**: zero-copy presentation builds
/// its renderer on the window compositor's adapter, so the window adapter wins
/// while zero-copy is active. The footer says so rather than implying otherwise.
pub(crate) fn gpu_panel(
    preference: &str,
    adapters: &[AdapterSummary],
    in_use: Option<&AdapterSummary>,
    scroll: &ScrollHandle,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    // An invalid stored value is what the viewport falls back from, so `Auto` is
    // the honest active row in that case.
    let parsed = AdapterChoice::parse(preference).ok();
    let auto_active = parsed.is_none() || parsed.as_ref() == Some(&AdapterChoice::Auto);
    let mut rows = div()
        .flex()
        .flex_col()
        .child(
            div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(format!("setting: {preference}")),
        );

    rows = rows.child(gpu_row(
        "dev-gpu-auto",
        "Auto",
        "let wgpu pick",
        auto_active,
        "ferrocad-gpu:auto".to_string(),
        cx,
    ));

    for (index, adapter) in adapters.iter().enumerate() {
        let active = match parsed.as_ref() {
            Some(AdapterChoice::Name(name)) => name.eq_ignore_ascii_case(&adapter.name),
            _ => false,
        };
        let detail = format!(
            "{} · {} · {}",
            adapter.backend, adapter.device_type, adapter.driver
        );
        rows = rows.child(gpu_row(
            &format!("dev-gpu-adapter-{index}"),
            &adapter.name,
            &detail,
            active,
            format!("ferrocad-gpu:adapter:{}", adapter.name),
            cx,
        ));
    }

    if adapters.is_empty() {
        rows = rows.child(
            div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(rgb(MUTED))
                .child("No adapters enumerated"),
        );
    }

    let in_use = match in_use {
        Some(adapter) => format!("in use: {} ({})", adapter.name, adapter.backend),
        None => "in use: no viewport".to_string(),
    };
    rows = rows
        .child(
            div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(rgb(MUTED))
                .debug_selector(|| "gpu-in-use".to_string())
                .child(in_use),
        )
        .child(
            div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(
                    "Click a row to select it. Zero-copy presentation keeps the window \
                     adapter; the choice applies to the image path.",
                ),
        );

    div()
        .id("dev-gpu")
        .debug_selector(|| "gpu-panel".to_string())
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .bg(rgb(PANEL))
        .child(scroll_area("dev-gpu", scroll, rows))
}

/// One GPU adapter row: label, backend/device/driver detail, and an `active` marker.
fn gpu_row(
    element_id: &str,
    label: &str,
    detail: &str,
    active: bool,
    action: String,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let element_id = element_id.to_string();
    let selector = element_id.clone();
    let active_selector = format!("{element_id}-active");
    let mut row = div()
        .id(SharedString::from(element_id))
        .debug_selector(move || selector)
        .flex()
        .flex_col()
        .px_2()
        .py_1()
        .text_xs()
        .text_color(rgb(if active { FG } else { MUTED }))
        .when(active, |el| el.bg(rgb(ACCENT)))
        .cursor_pointer()
        .hover(|style| style.bg(rgb(PANEL_ALT)))
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.run_command(&action, cx);
        }))
        .child(label.to_string());
    if !detail.is_empty() {
        row = row.child(div().text_color(rgb(MUTED)).child(detail.to_string()));
    }
    if active {
        row = row.child(
            div()
                .debug_selector(move || active_selector)
                .text_color(rgb(CHROME_ACTIVE))
                .child("active"),
        );
    }
    row
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
        .bg(rgba(DOCK_BG))
        .border_1()
        .border_color(rgba(DOCK_BORDER));

    // The dock is the **creation + global** surface: object verbs live in the
    // in-situ radial menu, so this strip never swaps width when the selection
    // changes.
    let mut rendered = false;
    for command in dock {
        // A dev-only command is a diagnostics entry: the Dev Tools list and the app
        // menu show it, a runtime surface never does (the same rule
        // `popover_commands` applies to a header menu). Without this, a command whose
        // `FerroCAD_Surface` falls back to the dock would appear over the canvas.
        if command.dev_only {
            continue;
        }
        rendered = true;

        let id = command.id.clone();
        let enabled = command.enabled;
        let checked = command.checked;
        // The dock is a command strip, not a checklist: checked/live state is
        // **accent ink**, and the background is reserved for an additive hover.
        let ink = if !enabled {
            MUTED
        } else if checked {
            ACTIVE_ACCENT
        } else {
            FG
        };
        let hover_ink = if checked { ACTIVE_ACCENT } else { FG };
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
            .text_color(rgb(ink))
            .when(enabled, |el| {
                el.cursor_pointer().hover(move |style| {
                    style.bg(rgba(ROW_HOVER)).text_color(rgb(hover_ink))
                })
            })
            .child(command.label().to_string());
        if enabled {
            button = button.on_click(cx.listener(move |this, _event, _window, cx| {
                this.run_command(&id, cx);
            }));
        }
        panel = panel.child(button);
    }

    if rendered {
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
            .hover(|style| style.bg(rgba(ROW_HOVER)).text_color(rgb(FG)))
            .child("Search (Ctrl+K)")
            .on_click(cx.listener(|this, _event, _window, cx| this.open_palette(cx))),
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

/// The command ids owned by a backend that is **not** the active one for its feature,
/// mapped to the feature it serves.
///
/// This is the shell-side half of "one provider per feature": the registry already
/// names the command each backend owns, so the losing backend's entry can be disabled
/// in the Dev Tools command list instead of looking equally runnable. The registry is
/// advisory — a mod that ignores `activeProvider` can still draw — so this is a UI
/// filter, not an execution gate.
pub(crate) fn inactive_provider_commands(features: &[ProviderFeature]) -> HashMap<String, String> {
    let mut inactive = HashMap::new();
    for feature in features {
        for provider in &feature.providers {
            if provider.active {
                continue;
            }
            if let Some(command) = provider.command.as_deref() {
                if !command.is_empty() {
                    inactive.insert(command.to_string(), feature.feature.clone());
                }
            }
        }
    }
    inactive
}

/// The Dev Tools command list: **every** registered command, vertically, with its
/// live enablement and the surface that owns it (T6's safety net).
///
/// `inactive` comes from [`inactive_provider_commands`]: a command whose provider is
/// not the active backend for its feature is shown but not runnable.
pub(crate) fn command_list_panel(
    model: &ShellModel,
    inactive: &HashMap<String, String>,
    scroll: &ScrollHandle,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let mut rows = div().flex().flex_col();

    for command in &model.commands {
        let id = command.id.clone();
        let inactive_feature = inactive.get(&command.id).cloned();
        // A command the losing provider owns stays listed (Dev Tools is the full
        // registry) but loses its click and its normal ink.
        let enabled = command.enabled && inactive_feature.is_none();
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
        let mut tip = command.tooltip.clone();
        if let Some(feature) = inactive_feature.as_deref() {
            if !tip.is_empty() {
                tip.push_str("  ·  ");
            }
            tip.push_str(&format!(
                "Inactive: another {feature} provider is selected (switch it in Providers)"
            ));
        }
        if !command.accel.is_empty() {
            if !tip.is_empty() {
                tip.push_str("  ·  ");
            }
            tip.push_str(&command.accel);
        }
        if !tip.is_empty() {
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
        .h_full()
        .min_h_0()
        .bg(rgb(PANEL))
        .debug_selector(|| "dev-command-list".to_string())
        .child(scroll_area("dev-commands", scroll, rows))
}

/// The status bar's literal bug: toggles the Dev Tools window.
///
/// The bug is a painted [`icon::bug`] (no font dependency), inked with the accent
/// while the window is open. The status bar is its home because Dev Tools is a tool
/// for the shell, not a modelling verb the viewport dock should carry.
pub(crate) fn dev_tools_button(active: bool, cx: &mut Context<Shell>) -> impl IntoElement {
    let tip: SharedString = if active {
        "Dev Tools: open (Ctrl+Shift+D)"
    } else {
        "Dev Tools (Ctrl+Shift+D)"
    }
    .into();
    div()
        .id("dev-tools-bug")
        .debug_selector(|| "dev-tools-bug".to_string())
        .w(px(22.))
        .h(px(18.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .cursor_pointer()
        .hover(|style| style.bg(rgb(PANEL)))
        .on_click(cx.listener(|this, _event, _window, cx| {
            this.run_command(DEV_TOOLS, cx);
        }))
        .tooltip(move |_window, cx| cx.new(|_| CommandTooltip { text: tip.clone() }).into())
        .child(icon::bug("dev-tools-icon", active))
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
    use gpui::{Modifiers, TestAppContext, VisualTestContext};

    /// Paint the window once so a following `simulate_*` has hitboxes to dispatch to.
    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    /// Click the centre of the element with `selector` (it must have been drawn).
    fn click_at(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} has no bounds"));
        cx.simulate_click(bounds.center(), Modifiers::default());
    }

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
                    command: None,
                },
                crate::python::GridProvider {
                    backend: "coin".to_string(),
                    label: "Coin (WGSL)".to_string(),
                    active: false,
                    command: None,
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
                    command: None,
                },
                crate::python::GridProvider {
                    backend: "coin".to_string(),
                    label: "Coin (WGSL)".to_string(),
                    active: false,
                    command: None,
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
                command: None,
            },
            crate::python::GridProvider {
                backend: "coin".to_string(),
                label: String::new(),
                active: false,
                command: None,
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

    /// The losing backend's command is filtered out of the runnable set; the active
    /// backend's command and a provider with no command contribute nothing.
    #[test]
    fn the_inactive_providers_command_is_filtered() {
        let features = vec![
            ProviderFeature {
                feature: "grid".to_string(),
                providers: vec![
                    crate::python::GridProvider {
                        backend: "ferrocad".to_string(),
                        label: "FerroCAD (native)".to_string(),
                        active: true,
                        command: Some("GridOverlay_Settings".to_string()),
                    },
                    crate::python::GridProvider {
                        backend: "coin".to_string(),
                        label: "Coin (WGSL)".to_string(),
                        active: false,
                        command: Some("GridOverlayCoin_Settings".to_string()),
                    },
                ],
            },
            ProviderFeature {
                feature: "gizmo".to_string(),
                providers: vec![crate::python::GridProvider {
                    backend: "ferrocad".to_string(),
                    label: "FerroCAD (native)".to_string(),
                    active: true,
                    command: None,
                }],
            },
        ];
        let inactive = inactive_provider_commands(&features);
        assert_eq!(
            inactive.get("GridOverlayCoin_Settings").map(String::as_str),
            Some("grid"),
            "the losing backend's command names its feature"
        );
        assert!(
            !inactive.contains_key("GridOverlay_Settings"),
            "the active backend's command stays runnable"
        );
        assert_eq!(
            inactive.len(),
            1,
            "a provider that owns no command contributes nothing"
        );
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

    /// A gizmo feature with the native backend active and the shader backend
    /// offered, the shape `python::provider_features()` returns.
    fn gizmo_feature() -> ProviderFeature {
        ProviderFeature {
            feature: "gizmo".to_string(),
            providers: vec![
                crate::python::GridProvider {
                    backend: "ferrocad".to_string(),
                    label: "FerroCAD (native)".to_string(),
                    active: true,
                    command: Some("GizmoOverlay_Settings".to_string()),
                },
                crate::python::GridProvider {
                    backend: "ferrocad-shader".to_string(),
                    label: "FerroCAD (shader)".to_string(),
                    active: false,
                    command: Some("GizmoOverlayShader_Settings".to_string()),
                },
            ],
        }
    }

    /// The registry entries the Gizmo popover needs, with the toggle's check mark
    /// supplied by the caller.
    fn gizmo_commands(checked: bool) -> Vec<CommandInfo> {
        vec![
            CommandInfo {
                id: GIZMO.to_string(),
                menu: "Gizmo".to_string(),
                checkable: true,
                checked,
                enabled: true,
                ..Default::default()
            },
            CommandInfo {
                id: "GizmoOverlay_Settings".to_string(),
                menu: "Gizmo".to_string(),
                enabled: true,
                ..Default::default()
            },
            CommandInfo {
                id: "GizmoOverlayShader_Settings".to_string(),
                menu: "Gizmo (shader)".to_string(),
                enabled: true,
                ..Default::default()
            },
            CommandInfo {
                id: RESET_GIZMO.to_string(),
                menu: "Reset gizmo appearance".to_string(),
                enabled: true,
                ..Default::default()
            },
        ]
    }

    /// The caret lists the two gizmo backends, then the show/settings/reset rows,
    /// with the same state-row semantics as Grid and Axes.
    #[test]
    fn the_gizmo_popover_lists_the_providers_the_toggle_the_settings_row_and_the_reset() {
        let feature = gizmo_feature();
        let rows = gizmo_rows(Some(&feature), &gizmo_commands(true));
        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "ferrocad-provider:gizmo:ferrocad",
                "ferrocad-provider:gizmo:ferrocad-shader",
                GIZMO,
                "GizmoOverlay_Settings",
                RESET_GIZMO,
            ]
        );

        assert_eq!(rows[0].label, "FerroCAD (native)");
        assert!(rows[0].checked, "the active backend is checked");
        assert!(
            rows[0].toggle && rows[0].keep_open,
            "a provider choice is a state row"
        );
        assert_eq!(rows[1].label, "FerroCAD (shader)");
        assert!(!rows[1].checked);

        assert_eq!(rows[2].label, "Show gizmo");
        assert!(rows[2].checked, "an enabled gizmo reads as checked");
        assert!(
            rows[2].toggle && rows[2].keep_open,
            "the toggle keeps the menu open"
        );
        assert!(
            rows[2].divider_before,
            "a hairline separates the mode group from the commands"
        );

        assert_eq!(rows[3].label, "Gizmo settings…");
        assert!(!rows[3].toggle && !rows[3].keep_open, "a one-shot action");

        assert_eq!(rows[4].label, "Reset to edition defaults");
        assert!(!rows[4].toggle && !rows[4].keep_open);
    }

    /// The check mark follows the command's `IsChecked`, so a disabled gizmo reads
    /// unchecked; without a provider feature only the command rows remain.
    #[test]
    fn a_disabled_gizmo_reads_as_unchecked_in_the_gizmo_popover() {
        let rows = gizmo_rows(None, &gizmo_commands(false));
        assert_eq!(
            rows.len(),
            2,
            "no provider rows and no settings row without a feature"
        );
        assert_eq!(rows[0].id, GIZMO);
        assert_eq!(rows[0].label, "Show gizmo");
        assert!(!rows[0].checked, "a disabled gizmo is unchecked");
        assert!(rows[0].toggle && rows[0].keep_open);
        assert_eq!(rows[1].id, RESET_GIZMO);
    }

    /// `Gizmo settings…` runs the active backend's settings command, which is the
    /// one path that selects `Settings/Gizmo` (the axis/grid rows' pattern).
    #[test]
    fn the_gizmo_settings_row_follows_the_active_backend() {
        let mut feature = gizmo_feature();
        feature.providers[0].active = false;
        feature.providers[1].active = true;
        let rows = gizmo_rows(Some(&feature), &gizmo_commands(true));
        let settings = rows
            .iter()
            .find(|row| row.label == "Gizmo settings…")
            .expect("the settings row");
        assert_eq!(settings.id, "GizmoOverlayShader_Settings");
    }

    /// The info row names the active backend and falls back to the backend id when
    /// the provider advertises no label; no feature and no provider each read as an
    /// honest empty state.
    #[test]
    fn the_gizmo_info_names_the_active_provider_and_says_when_there_is_none() {
        assert!(gizmo_info(None).is_empty());

        let feature = gizmo_feature();
        let info = gizmo_info(Some(&feature));
        assert_eq!(info.len(), 1);
        assert_eq!(info[0].id, "provider");
        assert_eq!(info[0].label, "Provider: FerroCAD (native)");

        let empty = ProviderFeature {
            feature: "gizmo".to_string(),
            providers: Vec::new(),
        };
        let info = gizmo_info(Some(&empty));
        assert_eq!(info[0].id, "no-provider");
        assert_eq!(info[0].label, "No gizmo provider");

        let unlabelled = ProviderFeature {
            feature: "gizmo".to_string(),
            providers: vec![crate::python::GridProvider {
                backend: "ferrocad-shader".to_string(),
                label: String::new(),
                active: true,
                command: None,
            }],
        };
        assert_eq!(
            gizmo_info(Some(&unlabelled))[0].label,
            "Provider: ferrocad-shader"
        );
    }

    /// The header builds with the Gizmo control between `WP` and `Snap`, and its
    /// caret opens the popover with the expected rows.
    #[gpui::test]
    fn the_header_builds_with_the_gizmo_control(cx: &mut TestAppContext) {
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let model = Shell::boot().expect("boot shell");
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        draw(&mut cx);

        assert!(cx.debug_bounds("viewport-header").is_some());
        let wp = cx.debug_bounds("wp-label").expect("the workplane control");
        let gizmo = cx.debug_bounds("gizmo-label").expect("the gizmo control");
        let snap = cx.debug_bounds("snap-label").expect("the snap control");
        assert!(
            wp.origin.x < gizmo.origin.x && gizmo.origin.x < snap.origin.x,
            "Gizmo sits between WP and Snap"
        );
        assert!(
            cx.debug_bounds("gizmo-state-dot").is_some(),
            "the pill carries the on/off dot"
        );

        click_at(&mut cx, "gizmo-caret");
        draw(&mut cx);
        assert!(
            cx.debug_bounds("gizmo-popover").is_some(),
            "the caret opens the gizmo popover"
        );
        for selector in [
            "gizmo-row-provider",
            "gizmo-row-ferrocad-provider:gizmo:ferrocad",
            "gizmo-row-App_ToggleGizmo",
            "gizmo-row-App_ResetGizmoAppearance",
        ] {
            assert!(
                cx.debug_bounds(selector).is_some(),
                "{selector} is rendered"
            );
        }
        assert!(
            cx.debug_bounds("gizmo-row-GizmoOverlay_Settings").is_some()
                || cx
                    .debug_bounds("gizmo-row-GizmoOverlayShader_Settings")
                    .is_some(),
            "the active backend's settings row is rendered"
        );
    }
}
