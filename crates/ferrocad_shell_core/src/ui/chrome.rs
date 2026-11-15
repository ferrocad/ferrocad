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
    Anchor, AnyElement, App, ClickEvent, Context, FontWeight, IntoElement, MouseButton,
    MouseDownEvent, Pixels, Point, Render, ScrollHandle, SharedString, Window, anchored, deferred,
    div, point, prelude::*, px, rgb, rgba,
};

use std::collections::{HashMap, HashSet};

use ferrocad_stage::{DressUpKind, GizmoMode, ToolStatus};
use ferrocad_wgpu::{AdapterChoice, AdapterSummary};
use ferrocad_widgets::icon;
use ferrocad_widgets::menu::{MenuInfoRow, MenuRow, MenuTab, menu_with_tabs};
use ferrocad_widgets::scroll::scroll_area;

use ferrocad_theme::ui::{
    ACTIVE_ACCENT, CHROME_ACTIVE, CHROME_ACTIVE_BG, CHROME_LINE, DOCK_BG, DOCK_BORDER, GLASS,
    INACTIVE_STRUCTURAL, ROW_HOVER, WHEEL_GLASS,
};
use crate::commands::{self, CommandInfo, Surface};
use crate::types::{AxisSummary, GridSummary, ProviderFeature};
use crate::shell::{
    ACCENT, CREATION_KEYS, CreationDock, FG, Loadout, MUTED, PANEL, PANEL_ALT, SelectFilter, Shell,
    ShellModel, Slot,
};

use super::command_icons;

/// The header controls' primary command ids.
pub const GRID: &str = "App_ToggleGrid";
pub const AXIS: &str = "App_ToggleAxisCross";
/// The gizmo split control's primary command: show or hide the transform gizmo.
pub const GIZMO: &str = "App_ToggleGizmo";
pub const WORKPLANE: &str = "App_ViewWorkplane";
/// The `WP` popover's grouping key. `WP` is a **selector**, not a command: it has no
/// primary action to run, so its rows group under a surface name rather than under a
/// primary command id (`WORKPLANE` above is only the control's registry gate).
pub const WORKPLANE_GROUP: &str = "App_Workplane";
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
    /// The curated icon key for the row's command, or empty when it has none.
    icon: String,
    /// The row represents a boolean state, so it carries a status dot (a provider
    /// choice and a command toggle both do; a one-shot action does not).
    toggle: bool,
    /// A **state** row (a command toggle or a provider choice) keeps the popover open
    /// after the click, so several options can be set in one visit and the dots
    /// follow. A one-shot action row still dismisses it.
    keep_open: bool,
    accel: String,
    /// The row's **selection key** (`1`, `2`, …), empty when it has none.
    ///
    /// Only the creation flyout uses it: a category is a short list, so it is also a
    /// numbered one, and the number is what picks the tool into its pill.
    key: String,
    /// Draw a 1 px hairline above this row (a group boundary).
    divider_before: bool,
}

/// The curated glyph for a command, or `None` when the note has not chosen one.
fn command_glyph(command: &CommandInfo, size: f32, ink: u32) -> Option<AnyElement> {
    command_icons::resolve(command).and_then(|key| icon::named(&key, size, ink))
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
            icon: command_icons::resolve(command).unwrap_or_default(),
            toggle: command.checkable,
            keep_open: command.checkable,
            accel: command.accel.clone(),
            key: String::new(),
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
fn provider_rows(feature: &str, providers: &[crate::types::GridProvider]) -> Vec<PopoverRow> {
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
            icon: String::new(),
            // A provider group is a radio set: the dot shows the active backend. It
            // is a **state** row like a checkbox, so the menu stays open and the dot
            // follows; `activate_provider` refreshes the summary in place.
            toggle: true,
            keep_open: true,
            accel: String::new(),
            key: String::new(),
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
            icon: command_icons::resolve(command).unwrap_or_default(),
            toggle: false,
            keep_open: false,
            accel: command.accel.clone(),
            key: String::new(),
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
/// The active construction plane, for the `WP` control.
///
/// A viewport always has one — the world `XY` plane is the default — so this is
/// present whenever a viewport exists. The dot appears only for a non-default plane.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct WorkplaneBadge {
    /// What the plane is called (`"XY"`, `"Face3"`, `"Datum"`, `"XZ"`).
    pub label: String,
    /// Whether it is anything other than the default world `XY` plane.
    pub custom: bool,
}

pub(crate) fn viewport_header(
    model: &ShellModel,
    grid: Option<&GridSummary>,
    axis: Option<&AxisSummary>,
    workplane: Option<&WorkplaneBadge>,
    loadout: Loadout,
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
        .map(|summary| {
            // B17a: a backend the user hid in Settings/About is not offered here either.
            let providers = crate::shell::prefs::visible_providers(
                "grid",
                &summary.providers,
                &model.prefs.prefs,
            );
            provider_rows("grid", &providers)
        })
        .unwrap_or_default();
    let axis_providers = axis
        .map(|summary| {
            let providers = crate::shell::prefs::visible_providers(
                "axis",
                &summary.providers,
                &model.prefs.prefs,
            );
            provider_rows("axis", &providers)
        })
        .unwrap_or_default();
    // The gizmo has no summary bridge and the model does not carry the provider
    // list, so read the registry while the popover is open; a bridge failure
    // leaves the rows out.
    let gizmo_feature = if gizmo_open {
        crate::app::provider_features(cx)
            .ok()
            .map(|features| crate::shell::prefs::visible_features(&features, &model.prefs.prefs))
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
    if commands::find(&model.commands, WORKPLANE).is_some() {
        controls = controls.child(workplane_button(workplane, wp_open, anchor, model, cx));
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
    // The topology filter: which element a click reports. It is a selector, not a
    // toggle, and it writes the same store as the bottom-left loadout bar.
    controls = controls.child(select_button(loadout, open == Some("select"), anchor, cx));
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
    // File verbs (`Open…`, `Save As…`) are the host's: they have no viewport
    // surface, so the app menu is where they are discoverable. Any command whose
    // `Group` is `File` joins them, so a mod's file command lands here too.
    for command in model.commands.iter().filter(|command| command.group == "File") {
        rows.push(PopoverRow::of(command));
    }
    // About is app-level like the Dev Tools toggle: no viewport surface, and its id is
    // synthetic (`ferrocad-about`) because it is shell chrome rather than a registry
    // command — so it adds no `catalog.json` row and no per-host registry entry.
    rows.push(PopoverRow {
        id: crate::shell::panels::about::OPEN_ID.to_string(),
        label: "About FerroCAD".to_string(),
        enabled: true,
        icon: String::new(),
        checked: false,
        toggle: false,
        keep_open: false,
        accel: String::new(),
        key: String::new(),
        divider_before: !rows.is_empty(),
    });
    if let Some(command) = commands::find(&model.commands, DEV_TOOLS) {
        rows.push(PopoverRow {
            id: command.id.clone(),
            label: command.label().to_string(),
            enabled: command.enabled,
            icon: command_icons::resolve(command).unwrap_or_default(),
            // The registry cannot know whether a window exists; the shell can.
            checked: dev_tools_open,
            // A toggle, but the app menu is a launcher: clicking closes it so the
            // window it just opened is visible.
            toggle: true,
            keep_open: false,
            accel: command.accel.clone(),
            key: String::new(),
            // The diagnostics row is a different section from the file verbs.
            divider_before: !rows.is_empty(),
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
            .child(
                div()
                    .debug_selector(|| "app-menu-icon".to_string())
                    .flex()
                    .items_center()
                    // The glyph is an embedded SVG; the text mark is a fallback only
                    // for a build whose asset went missing.
                    .child(
                        icon::named("menu", icon::CONTROL, FG)
                            .unwrap_or_else(|| div().child("☰").into_any_element()),
                    ),
            )
            // Keep the title-bar drag from swallowing the click.
            .on_mouse_down(MouseButton::Left, |_event, _window, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _event, _window, cx| this.open_popover("app", cx))),
    );

    if open {
        control = control.child(
            // The popover's height must not move its anchor. An anchored element's
            // static position comes from the flex container it sits in (here the
            // 32 px title bar, centred), so a taller menu would start above the
            // window and snap to the top margin over the bar. A zero-size absolute
            // wrapper pins the anchor to the control's origin instead.
            div().absolute().top(px(0.)).left(px(0.)).child(
                deferred(
                    anchored()
                        .anchor(Anchor::TopLeft)
                        .offset(point(px(0.), px(24.)))
                        .snap_to_window_with_margin(px(8.))
                        .child(popover("app", rows, cx)),
                )
                .with_priority(1),
            ),
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
/// The `WP` control: a **selector**, not a toggle.
///
/// It has no state dot — `Grid`, `Axes`, `Gizmo` and `Snap` are capabilities that can be
/// off, while a workplane always exists — and no primary action: the label carries the
/// state (`WP: XY` in normal ink, `WP: Face1` in `CHROME_ACTIVE`) and a click anywhere on
/// the button opens the datum menu. A split button would leave a dead primary click at the
/// default and a dot that is always lit.
fn workplane_button(
    badge: Option<&WorkplaneBadge>,
    open: bool,
    anchor: Option<Point<Pixels>>,
    model: &ShellModel,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let custom = badge.is_some_and(|badge| badge.custom);
    let label: SharedString = match badge {
        Some(badge) => format!("WP: {}", badge.label).into(),
        None => "WP".into(),
    };
    let rows: Vec<PopoverRow> =
        commands::popover_commands(&model.commands, Surface::Workplane, WORKPLANE_GROUP)
            .into_iter()
            .map(PopoverRow::of)
            .collect();

    let mut control = div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .rounded_sm()
        .when(open, |el| el.border_1().border_color(rgba(0xffffff33)));

    control = control.child(
        div()
            .id("wp-label")
            .debug_selector(|| "wp-label".to_string())
            .px_2()
            .py_1()
            .text_xs()
            .text_color(rgb(if custom { CHROME_ACTIVE } else { FG }))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(PANEL)))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(icon::workplane("wp", custom))
                    .child(label),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, _cx| {
                    this.set_popover_anchor(event.position);
                }),
            )
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.open_popover("wp", cx);
            })),
    );
    control = control.child(
        div()
            .id("wp-caret")
            .debug_selector(|| "wp-caret".to_string())
            .px_1()
            .py_1()
            .text_xs()
            .text_color(rgb(MUTED))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(PANEL)))
            .child("▾")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, _cx| {
                    this.set_popover_anchor(event.position);
                }),
            )
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.open_popover("wp", cx);
            })),
    );
    if open {
        let position = anchor
            .map(|pointer| point(pointer.x, pointer.y + px(18.)))
            .unwrap_or_else(|| point(px(0.), px(28.)));
        control = control.child(
            deferred(
                anchored()
                    .anchor(Anchor::TopLeft)
                    .position(position)
                    .snap_to_window_with_margin(px(8.))
                    .child(popover_with_info("wp", Vec::new(), rows, None, cx)),
            )
            .with_priority(1),
        );
    }
    control
}

/// The `Select ▾` topology filter control (U4).
///
/// A **selector**, like `WP`: it has no primary action, because equipping a slot is
/// what the bottom-left bar and the `0`–`3` keys do. The label carries the current
/// filter, accented when it overrides the slot's default, and the rows are the four
/// filters. A row id is synthetic (`select:<filter>`), intercepted by
/// [`Shell::run_command`] before the registry.
fn select_button(
    loadout: Loadout,
    open: bool,
    anchor: Option<Point<Pixels>>,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let filter = loadout.filter();
    let rows: Vec<PopoverRow> = SelectFilter::ALL
        .into_iter()
        .map(|candidate| PopoverRow {
            id: format!("select:{}", candidate.label().to_ascii_lowercase()),
            label: candidate.label().to_string(),
            enabled: true,
            checked: candidate == filter,
            icon: String::new(),
            // A radio set: the dot follows the active filter and the menu stays open
            // for a comparison, like a provider group.
            toggle: true,
            keep_open: true,
            accel: String::new(),
            key: String::new(),
            divider_before: false,
        })
        .collect();
    let label: SharedString = format!("Select: {}", filter.label()).into();

    let mut control = div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .rounded_sm()
        .when(open, |el| el.border_1().border_color(rgba(0xffffff33)));

    control = control.child(
        div()
            .id("select-label")
            .debug_selector(|| "select-label".to_string())
            .px_2()
            .py_1()
            .text_xs()
            .text_color(rgb(if loadout.overridden() {
                CHROME_ACTIVE
            } else {
                FG
            }))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(PANEL)))
            .child(label)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, _cx| {
                    this.set_popover_anchor(event.position);
                }),
            )
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.open_popover("select", cx);
            })),
    );
    control = control.child(
        div()
            .id("select-caret")
            .debug_selector(|| "select-caret".to_string())
            .px_1()
            .py_1()
            .text_xs()
            .text_color(rgb(MUTED))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(PANEL)))
            .child("▾")
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _window, _cx| {
                    this.set_popover_anchor(event.position);
                }),
            )
            .on_click(cx.listener(|this, _event, _window, cx| {
                this.open_popover("select", cx);
            })),
    );
    if open {
        let position = anchor
            .map(|pointer| point(pointer.x, pointer.y + px(18.)))
            .unwrap_or_else(|| point(px(0.), px(28.)));
        control = control.child(
            deferred(
                anchored()
                    .anchor(Anchor::TopLeft)
                    .position(position)
                    .snap_to_window_with_margin(px(8.))
                    .child(popover_with_info("select", Vec::new(), rows, None, cx)),
            )
            .with_priority(1),
        );
    }
    control
}

/// The loadout slot's glyph: the hand it equips.
///
/// Slot 1 hosts both handle sets, so it names the one the user will actually get —
/// `rotate-object` rings or `move` arrows — exactly as [`Slot::label_for`] does. Slot 3
/// names its dress-up the same way: a fillet's rounded corner or a chamfer's bevel.
fn loadout_icon(slot: Slot, mode: GizmoMode, dress_up: DressUpKind) -> &'static str {
    match (slot, mode) {
        (Slot::Move, GizmoMode::Rotate) => "rotate-object",
        (Slot::Move, _) => "move",
        (Slot::PushPull, _) => "arrow-up-down",
        (Slot::Fillet, _) => match dress_up {
            DressUpKind::Fillet => "corner-radius",
            DressUpKind::Chamfer => "bevel-gauge",
        },
        (Slot::Inspect, _) => "select-pointer",
    }
}

/// The loadout bar (U4): `[1 Move] [2 Push/Pull] [3 Fillet] [0 Idle]` over the
/// canvas' bottom-left corner. The pointer comes last because it is the neutral
/// hand, not a fourth tool. Slot 3 names the dress-up it currently arms (`3 Fillet`
/// or `3 Chamfer`), and pressing `3` again is the toggle.
///
/// It shares the creation dock's baseline and glass. The active slot is accented over
/// `CHROME_ACTIVE_BG`; a click equips it exactly as its key does, so the hotkey stays
/// discoverable without opening the wheel. The caller positions the pill in the
/// bottom-left column (the Move form docks directly above it), so this returns the pill
/// itself.
///
/// `blocked` dims the bar and drops every click while a move is pending: the hand cannot
/// change until the user finishes it with `✗`/`✓`.
pub(crate) fn loadout_bar(
    loadout: Loadout,
    mode: GizmoMode,
    blocked: bool,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let mut panel = div()
        .id("loadout-bar")
        .debug_selector(|| "loadout-bar".to_string())
        .occlude()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgba(DOCK_BG))
        .border_1()
        .border_color(rgba(DOCK_BORDER))
        .text_xs()
        .when(blocked, |el| el.opacity(0.45));
    let dress_up = loadout.dress_up();

    for slot in Slot::ALL {
        let active = loadout.slot() == slot;
        let key = slot.key();
        let mut button = div()
            .id(SharedString::from(format!("loadout-{key}")))
            .debug_selector(move || format!("loadout-{key}"))
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded_sm()
            .text_color(rgb(if active { CHROME_ACTIVE } else { FG }))
            .when(active, |el| el.bg(rgba(CHROME_ACTIVE_BG)))
            .child(div().text_color(rgb(MUTED)).child(key))
            .child(
                div()
                    .debug_selector(move || format!("loadout-{key}-icon"))
                    .flex()
                    .items_center()
                    .child(
                        icon::named(
                            loadout_icon(slot, mode, dress_up),
                            icon::ROW,
                            if active { CHROME_ACTIVE } else { FG },
                        )
                        .unwrap_or_else(|| div().into_any_element()),
                    ),
            )
            .child(slot.label_for(mode, dress_up));
        if !blocked {
            button = button
                .cursor_pointer()
                .hover(|style| style.bg(rgba(ROW_HOVER)))
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.activate_loadout_slot(slot, cx);
                }));
        }
        panel = panel.child(button);
    }

    panel
}

/// The Inspect hand's scope switcher: `Body | Face | Edge`, docked directly above the
/// loadout bar.
///
/// It is a second view of the same [`Loadout`] filter the header's `Select ▾` writes,
/// not a private mode: clicking a segment changes what the next pick reports and what
/// the stage highlight previews. `Auto` stays a header-only choice, so no segment is
/// lit while it is active.
pub(crate) fn inspect_bar(loadout: Loadout, cx: &mut Context<Shell>) -> AnyElement {
    let current = loadout.filter();
    let mut panel = div()
        .id("inspect-bar")
        .debug_selector(|| "inspect-bar".to_string())
        .occlude()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgba(DOCK_BG))
        .border_1()
        .border_color(rgba(DOCK_BORDER))
        .text_xs();

    // The three scopes that change what a pick reports. `Auto` is the pre-loadout
    // "whatever the pick reports" mode and remains in the header menu.
    for filter in [SelectFilter::Body, SelectFilter::Face, SelectFilter::Edge] {
        let active = current == filter;
        let label = filter.label();
        panel = panel.child(
            div()
                .id(SharedString::from(format!(
                    "inspect-{}",
                    label.to_ascii_lowercase()
                )))
                .debug_selector(move || format!("inspect-{}", label.to_ascii_lowercase()))
                .px_2()
                .py_1()
                .rounded_sm()
                .text_color(rgb(if active { CHROME_ACTIVE } else { FG }))
                .when(active, |el| el.bg(rgba(CHROME_ACTIVE_BG)))
                .cursor_pointer()
                .hover(|style| style.bg(rgba(ROW_HOVER)))
                .child(label)
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    this.set_select_filter(filter, cx);
                })),
        );
    }

    panel.into_any_element()
}

fn split_button(
    key: &'static str,
    label: impl Into<SharedString>,
    primary: &'static str,
    surface: Surface,
    open: bool,
    info: Vec<PopoverInfo>,
    providers: Vec<PopoverRow>,
    anchor: Option<Point<Pixels>>,
    stateful: bool,
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
        key, label, primary, open, info, rows, anchor, stateful, command, cx,
    )
}

/// A split button from an explicit row list.
///
/// [`split_button`] derives the rows from the registry surface; the Gizmo control
/// builds its own because its settings commands are dev-only and its provider
/// list is not a summary. The visual language is identical.
fn split_button_with_rows(
    key: &'static str,
    label: impl Into<SharedString>,
    primary: &'static str,
    open: bool,
    info: Vec<PopoverInfo>,
    rows: Vec<PopoverRow>,
    anchor: Option<Point<Pixels>>,
    stateful: bool,
    command: &CommandInfo,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let label: SharedString = label.into();
    // Dielectric: the pill's glass is the surface and the glyph ink carries on/off
    // (greyscale off, colour on). A checked control used to fill with `ACCENT`,
    // which pulled the eye away from the model (the NaviCube bar is the reference look).
    let mut control = div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .rounded_sm()
        .when(open, |el| el.border_1().border_color(rgba(0xffffff33)));

    let enabled = command.enabled;
    let glyph: Option<AnyElement> = match key {
        "grid" => Some(icon::grid(key, command.checked).into_any_element()),
        "axis" => Some(icon::axes(key, command.checked).into_any_element()),
        "gizmo" => Some(icon::gizmo(key, command.checked).into_any_element()),
        "snap" => Some(icon::snap(key, command.checked).into_any_element()),
        _ => None,
    };
    let mut row = div().flex().flex_row().items_center().gap_1();
    if let Some(glyph) = glyph {
        row = row.child(glyph);
    }
    row = row.child(label.clone());
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
        .child(row)
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
    if stateful {
        // The glyph ink is the on/off cue; the tooltip states it in words on hover
        // (the report was that hovering gave no indication).
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
                    .child(popover_with_info(key, info, rows, None, cx)),
            )
            .with_priority(1),
        );
    }
    control
}

/// An anchored popover: a plain column of command rows, dismissed by an outside
/// press. It has no backdrop, so the canvas beneath still receives pointer events.
fn popover(key: &'static str, rows: Vec<PopoverRow>, cx: &mut Context<Shell>) -> impl IntoElement {
    popover_with_info(key, Vec::new(), rows, None, cx)
}

/// A popover with read-only value rows above its commands (T4).
///
/// The panel is the reusable [`ferrocad_widgets::menu`]; this function is the shell
/// adapter: it maps registry commands to rows and turns a click into the command run.
/// A **state** row (`keep_open`) runs in place so the menu stays open and its dot
/// follows; an action row dismisses the menu.
/// `pass_through` names an overlay to report when a press dismisses the panel (the
/// creation flyout's negative space): the shell then adds `clicked through: <name>` to
/// the hover line, without teaching the viewport anything about chrome.
fn popover_with_info(
    key: &'static str,
    info: Vec<PopoverInfo>,
    rows: Vec<PopoverRow>,
    pass_through: Option<&'static str>,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    popover_with_tabs(key, info, Vec::new(), rows, pass_through, cx)
}

/// [`popover_with_info`] with a segmented tab bar above its rows.
///
/// The tabs are how a panel presents several short lists instead of one long one (the
/// creation flyout's categories); every other popover passes none, so its panel is the
/// same column it always was.
fn popover_with_tabs(
    key: &'static str,
    info: Vec<PopoverInfo>,
    tabs: Vec<MenuTab>,
    rows: Vec<PopoverRow>,
    pass_through: Option<&'static str>,
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
            icon: (!row.icon.is_empty()).then(|| row.icon.into()),
            enabled: row.enabled,
            checked: row.checked,
            toggle: row.toggle,
            accel: (!row.accel.is_empty()).then(|| row.accel.into()),
            key: (!row.key.is_empty()).then(|| row.key.into()),
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
    let on_tab = move |id: &str, _event: &ClickEvent, _window: &mut Window, cx: &mut App| {
        let id = id.to_string();
        let _ = shell.update(cx, |shell, cx| shell.set_creation_tab(&id, cx));
    };
    let shell = cx.entity().downgrade();
    let on_dismiss = move |_window: &mut Window, cx: &mut App| {
        let _ = shell.update(cx, |shell, cx| {
            if let Some(overlay) = pass_through {
                shell.note_pass_through(overlay);
            }
            shell.close_popover(cx)
        });
    };

    menu_with_tabs(key, info, tabs, rows, on_tab, on_row, on_dismiss)
}

/// The hairline that separates groups in the **vertical** globals dock.
fn dock_rule(id: usize) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("dock-rule-{id}")))
        .debug_selector(move || format!("dock-rule-{id}"))
        .h(px(1.))
        .my_1()
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


/// An action button's content: an embedded glyph and its word.
fn action_label(
    key: &'static str,
    label: &'static str,
    ink: u32,
    selector: &'static str,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .child(
            div()
                .debug_selector(move || selector.to_string())
                .flex()
                .items_center()
                .child(
                    icon::named(key, icon::ROW, ink).unwrap_or_else(|| div().into_any_element()),
                ),
        )
        .child(label)
}

/// Whether the armed tool has **begun** — its first click has placed the anchor (or its
/// first point).
///
/// This is the rule that keeps arming a tool quiet. Pressing a pill or a key equips the
/// slot and lets the canvas show the placement ghost of what the next click would make;
/// the completion row — the tool's name, its numbers and the `✗`/`✓` pair — is a response
/// to that click, not to equipping, so it only appears once there is something to
/// complete. Every tool reports the click through `ToolStatus::points`, including the
/// parameter tools: `BoxTool` anchors on the press and only then offers `W`/`L`/`H`, so the
/// numbers are typed *after* the placement rather than in front of an empty canvas.
pub(crate) fn tool_has_begun(status: &ToolStatus) -> bool {
    status.points > 0
}

/// The armed tool's row: what is running, its numbers (or its point count), its hint and
/// the `Cancel` / `Finish` pair — the **completion row**.
///
/// It is the row directly above the two hands' bar, in the same bottom-left column, so a
/// session never takes the loadout, the create dock or the globals away, and the row reads
/// as part of the same instrument. It only exists once [`tool_has_begun`]: an armed tool
/// that has placed nothing shows a hover ghost on the canvas and no row at all.
///
/// **Cancel** is neutral (no accent fill) and ends the session through
/// [`Shell::cancel_tool`]; **Finish** is the accent action and takes the same host path
/// as `Enter` ([`Shell::finish_active_tool`]), so the button and the key cannot drift.
pub(crate) fn tool_pill(
    status: &ToolStatus,
    edit: Option<&crate::shell::panels::tool_fields::ToolFieldEdit>,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let mut pill = div()
        .id("tool-strip")
        .debug_selector(|| "tool-strip".to_string())
        .occlude()
        .flex()
        .flex_row()
        .items_center()
        .gap_3()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgba(WHEEL_GLASS))
        .border_1()
        .border_color(rgba(DOCK_BORDER))
        .text_xs()
        .child(div().text_color(rgb(FG)).child(status.label.clone()));
    // The numeric row (C2): one tabular-monospace group, the fields separated by
    // hairlines, the unit printed once at the end. It is docked with the pill, so no
    // number follows the pointer.
    if !status.fields.is_empty() {
        let mut fields = div()
            .id("tool-fields")
            .debug_selector(|| "tool-fields".to_string())
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .font_family("monospace");
        let unit = status
            .fields
            .iter()
            .find(|field| !field.unit.is_empty())
            .map(|field| field.unit.clone())
            .unwrap_or_default();
        for (index, field) in status.fields.iter().enumerate() {
            let focused = edit.is_some_and(|edit| edit.index == index);
            let value = edit
                .filter(|edit| edit.index == index)
                .map(|edit| edit.text.clone())
                .unwrap_or_else(|| field.value.clone());
            let mut cell = div()
                .id(SharedString::from(format!("tool-field-{}", field.id)))
                .debug_selector({
                    let id = field.id.clone();
                    move || format!("tool-field-{id}")
                })
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_1()
                .rounded_sm()
                .child(div().text_color(rgb(MUTED)).child(field.label.clone()))
                .child(
                    div()
                        .debug_selector({
                            let id = field.id.clone();
                            move || format!("tool-field-{id}-value")
                        })
                        .text_color(rgb(if focused { ACTIVE_ACCENT } else { FG }))
                        .child(value),
                );
            if focused {
                // A caret, so "this field owns the keyboard" needs no focus ring.
                cell = cell.child(div().text_color(rgb(ACTIVE_ACCENT)).child("▏"));
            }
            fields = fields.child(cell);
        }
        if !unit.is_empty() {
            fields = fields.child(div().text_color(rgb(MUTED)).child(unit));
        }
        pill = pill.child(fields);
    } else if status.points > 0 {
        pill = pill.child(div().text_color(rgb(MUTED)).child(format!(
            "{} point{}",
            status.points,
            if status.points == 1 { "" } else { "s" }
        )));
    }
    // The row only exists once the tool has begun (`tool_has_begun`), so the pair is
    // always offered here: the user has placed a point or has a parameter row to finish.
    pill = pill.child(
        div()
            .min_w_0()
            .truncate()
            .text_color(rgb(MUTED))
            .child(status.hint.clone()),
    );
    pill
        // The abort action is neutral: the accent fill belongs to Finish.
        .child(
            div()
                .id("tool-cancel")
                .debug_selector(|| "tool-cancel".to_string())
                .px_2()
                .py_1()
                .rounded_sm()
                .bg(rgb(PANEL))
                .text_color(rgb(FG))
                .cursor_pointer()
                .hover(|style| style.bg(rgb(PANEL_ALT)))
                .child(action_label("close", "Cancel", FG, "tool-cancel-icon"))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.cancel_tool(cx);
                })),
        )
        // Finish is a real button, not a hint: a trackpad user can end the session without
        // finding `Enter`. It calls the same host path as the key.
        .child(
            div()
                .id("tool-finish")
                .debug_selector(|| "tool-finish".to_string())
                .px_2()
                .py_1()
                .rounded_sm()
                .bg(rgb(ACCENT))
                .text_color(rgb(FG))
                .cursor_pointer()
                .hover(|style| style.bg(rgb(PANEL_ALT)))
                .child(action_label("check", "Finish", FG, "tool-finish-icon"))
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.finish_active_tool(cx);
                })),
        )
        .into_any_element()
}

/// The globals dock: `Undo`, `Redo`, `Fit` and Search, as a vertical pill on the
/// canvas' **left edge**, centred down it.
///
/// **The strict contract.** It holds exactly the `dock`-surface globals and Search:
///
/// ```text
/// [ Undo ]
/// [ Redo ]
/// [ Fit  ]
/// [------]
/// [ Search (Ctrl+K) ]
/// ```
///
/// It never grows a list of creation or modification buttons — a creation tool declares
/// `FerroCAD_Surface: "creation"` and gets a pill of its own in the create dock beside the
/// Modify hand, a shape verb declares `"radial"` or `"palette"` and appears in the wheel or
/// only in Search. A dock full of verbs is a toolbar.
///
/// It is a column on the left because the bottom of the canvas belongs to the two hands:
/// the Modify bar and the create dock share one row down there, and the armed tool's pill
/// takes the centre. `blocked` (a pending transform or manipulator) dims the dock to 45 %
/// and drops its clicks, exactly as the hand column dims.
pub(crate) fn globals_dock(
    model: &ShellModel,
    blocked: bool,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let dock = model.surfaces().dock;
    let mut panel = div()
        .id("globals-dock")
        .debug_selector(|| "globals-dock".to_string())
        // The pill owns the hitbox: the wrapper below is only a placement box, so clicks
        // beside the glass still reach the canvas.
        .occlude()
        .flex_none()
        .flex()
        .flex_col()
        .items_start()
        .gap_1()
        .px_1()
        .py_1()
        .rounded_md()
        .bg(rgba(DOCK_BG))
        .border_1()
        .border_color(rgba(DOCK_BORDER))
        .text_xs()
        .when(blocked, |el| el.opacity(0.45));

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
        // The dock is a command strip, not a checklist: checked/live state is **accent
        // ink**, and the background is reserved for an additive hover.
        let ink = if !enabled {
            MUTED
        } else if checked {
            ACTIVE_ACCENT
        } else {
            FG
        };
        let hover_ink = if checked { ACTIVE_ACCENT } else { FG };
        // The glyph is a secondary mark beside the label, so it takes the same ink as the
        // text: muted when disabled, accented when the command is live.
        let glyph = command_glyph(command, icon::ROW, ink).map(|glyph| {
            div()
                .debug_selector({
                    let id = id.clone();
                    move || format!("dock-{id}-icon")
                })
                .flex()
                .items_center()
                .child(glyph)
        });
        let mut content = div().flex().flex_row().items_center().gap_1();
        if let Some(glyph) = glyph {
            content = content.child(glyph);
        }
        content = content.child(command.label().to_string());

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
            .child(content);
        if enabled && !blocked {
            button = button.on_click(cx.listener(move |this, _event, _window, cx| {
                this.run_command(&id, cx);
            }));
        }
        panel = panel.child(button);
    }

    if rendered {
        panel = panel.child(dock_rule(1));
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
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .debug_selector(|| "dock-search-icon".to_string())
                            .flex()
                            .items_center()
                            .child(
                                icon::named("search", icon::ROW, MUTED)
                                    .unwrap_or_else(|| div().into_any_element()),
                            ),
                    )
                    .child("Search (Ctrl+K)"),
            )
            .on_click(cx.listener(|this, _event, _window, cx| this.open_palette(cx))),
    );

    // The placement box: full height on the left edge, transparent and with no hitbox of
    // its own, so the canvas behind it still takes the pointer. The pill inside is
    // content-sized and centred vertically, above the hand column at the bottom.
    div()
        .absolute()
        .left(px(16.))
        .top(px(0.))
        .bottom(px(0.))
        .flex()
        .flex_col()
        .justify_center()
        .child(panel)
}

/// The creation flyout as its own overlay, anchored at the pointer that opened it.
///
/// It used to hang off the bottom-centre column; now that the create dock lives beside the
/// Modify hand, the flyout is a canvas overlay like the wheel.
pub(crate) fn creation_flyout_overlay(
    model: &ShellModel,
    creation: &CreationDock,
    anchor: Option<Point<Pixels>>,
    cx: &mut Context<Shell>,
) -> Option<AnyElement> {
    let groups = model.surfaces();
    creation_flyout(&groups.creation, creation, model, anchor, cx)
}

/// How many tools a category's flyout numbers. A category is a short list, and the top
/// digit row runs out at nine.
pub(crate) const CREATION_ROW_KEYS: usize = 8;

/// The five creation tiers, in the order the flyout lists them.
///
/// The design's hierarchy (`creation-loadout.md` §1) is Spatial first — you draw
/// before you solidify — and a mod that invents a sixth category sorts after all five
/// rather than disappearing.
pub(crate) const CREATION_CATEGORIES: [&str; 5] = ["Sketch", "Solid", "Surface", "Text", "Mesh"];

pub(crate) fn creation_rank(category: Option<&str>) -> usize {
    category
        .and_then(|category| {
            CREATION_CATEGORIES
                .iter()
                .position(|known| known.eq_ignore_ascii_case(category))
        })
        .unwrap_or(CREATION_CATEGORIES.len())
}

/// The selection's topology, as a [`TopologyGate`](commands::TopologyGate) sees it:
/// the reference count and the kind (faces/edges win over a bare body, because that is
/// what the user picked).
pub(crate) fn selection_topology(
    selection: &[crate::selection::SelectionItem],
) -> (usize, commands::ElementKind) {
    let kind = selection
        .iter()
        .find_map(|item| item.elements.first())
        .map(|element| commands::ElementKind::of_element(Some(element.as_str())))
        .unwrap_or(commands::ElementKind::Body);
    (selection.len(), kind)
}

/// Whether a creation command's topology gate is satisfied right now.
fn gate_satisfied(model: &ShellModel, command: &CommandInfo) -> bool {
    let Some(gate) = command.topology_gate() else {
        return true;
    };
    let (count, kind) = selection_topology(&model.selection);
    gate.satisfied(count, kind)
}

/// The categories an edition registers a creation tool for, in the design's order.
///
/// One pill per entry: a category with no tool is not a pill and not a tab, so an
/// edition that draws nothing has no create dock at all.
fn creation_categories<'a>(creation: &[&'a CommandInfo]) -> Vec<&'a str> {
    let mut categories: Vec<&'a str> = Vec::new();
    for command in creation.iter().filter(|command| !command.dev_only) {
        if let Some(name) = command.category()
            && !categories
                .iter()
                .any(|known| known.eq_ignore_ascii_case(name))
        {
            categories.push(name);
        }
    }
    categories.sort_by_key(|name| creation_rank(Some(name)));
    categories
}

/// The tool each category's pill names: its memory when that still names a live tool of
/// the category, else the category's first tool in registry order.
pub(crate) fn creation_pill_tools<'a>(
    creation: &[&'a CommandInfo],
    dock: &CreationDock,
) -> Vec<(String, &'a CommandInfo)> {
    creation_categories(creation)
        .into_iter()
        .filter_map(|category| {
            let mut tools = creation
                .iter()
                .copied()
                .filter(|command| !command.dev_only)
                .filter(|command| {
                    command
                        .category()
                        .is_some_and(|name| name.eq_ignore_ascii_case(category))
                });
            let tool = dock
                .memory(category)
                .and_then(|id| tools.clone().find(|command| command.id == id))
                .or_else(|| tools.next())?;
            Some((category.to_string(), tool))
        })
        .collect()
}

/// The create dock: one pill per creation category, in the bottom row right after the
/// Modify hand's bar.
///
/// It is a **loadout**, not a menu: each pill names the tool its category was left on, so
/// an edition's whole creation set is visible in one row and one click away. The row is
/// permanent — there is no control that folds it away, because a loadout that can be put
/// away is not a loadout, and the six keys that address it would then address nothing.
///
/// It is deliberately **not** part of the globals dock: that pill holds `Undo`/`Redo`/`Fit`
/// and Search and nothing else, so the globals never change width with an edition.
pub(crate) fn create_dock(
    creation: &[&CommandInfo],
    dock: &CreationDock,
    model: &ShellModel,
    blocked: bool,
    holding: bool,
    cx: &mut Context<Shell>,
) -> Option<AnyElement> {
    let pills = creation_pill_tools(creation, dock);
    if pills.is_empty() {
        return None;
    }
    let mut row = div()
        .id("create-dock")
        .debug_selector(|| "create-dock".to_string())
        .occlude()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .rounded_md()
        .bg(rgba(DOCK_BG))
        .border_1()
        .border_color(rgba(DOCK_BORDER))
        .text_xs()
        .when(blocked, |el| el.opacity(0.45));
    for (index, (category, command)) in pills.iter().enumerate() {
        let open = dock
            .tab()
            .is_some_and(|tab| tab.eq_ignore_ascii_case(category));
        // The pill holding the Add hand's armed tool is the equipped slot, so it lights up
        // like the Modify bar's hand does.
        let armed = dock.active() == Some(command.id.as_str());
        row = row.child(creation_pill(
            index, category, command, open, armed, model, blocked, holding, cx,
        ));
    }
    Some(row.into_any_element())
}

/// One creation pill: `5 ⊕ <Category>: <Tool> ▾`.
///
/// The category is the quiet half and the tool is the loud one, so the row reads as a
/// set of loadouts (`Sketch: Rectangle`, `Solid: Box`) rather than as four buttons with
/// two words each. The key leads, muted, exactly as it does on the Modify hand's bar
/// (`1 Move`): both hands are one digit space, and the strip says so.
///
/// The left half is the pill's **slot**: it goes through
/// [`Shell::activate_creation_slot`](crate::shell::Shell), so a click and the pill's key
/// are the same gesture — arm the tool it names, or step to the next tool of the category
/// when this pill already holds the armed one. The caret opens the flyout.
///
/// `armed` is the equipped slot — this pill holds the tool the Add hand armed last — and
/// it takes the accent the Modify bar gives its hand.
#[allow(clippy::too_many_arguments)]
fn creation_pill(
    index: usize,
    category: &str,
    command: &CommandInfo,
    open: bool,
    armed: bool,
    model: &ShellModel,
    blocked: bool,
    holding: bool,
    cx: &mut Context<Shell>,
) -> AnyElement {
    // A tool whose selection-dependent gate is unmet stays **visible and clickable**: a
    // click reports what it needs rather than doing nothing, which is the same refusal
    // the flyout's row shows. A command disabled by its own `IsActive` is not clickable.
    // The slot itself re-checks the gate, because it may step to a *different* tool of the
    // category.
    let gated = !gate_satisfied(model, command);
    let arm_enabled = command.enabled && !blocked;
    let ink = if armed {
        CHROME_ACTIVE
    } else if gated {
        INACTIVE_STRUCTURAL
    } else if arm_enabled {
        FG
    } else {
        MUTED
    };
    // The pill's hotkey, by position. It is what the Modify hand's bar puts in front of
    // its label (`1 Move`), so `5 Sketch: Rectangle` reads as the same kind of slot.
    let key = CREATION_KEYS.get(index).copied().unwrap_or_default();

    let container = div()
        .id(SharedString::from(format!("creation-pill-{index}")))
        .debug_selector({
            let selector = format!("creation-pill-{index}");
            move || selector
        })
        .flex()
        .flex_row()
        .items_center()
        .rounded_sm()
        // `PILL_HOLD`: the accent fill lingers for a moment after a creation commit.
        // The equipped slot keeps the same wash for as long as its tool is the armed one
        // (the Modify bar's lit hand), so the fill means "this is the slot in hand" and
        // the linger is only its tail. Decoration either way: the strip takes every click.
        .when((armed || holding) && !blocked, |el| {
            el.bg(rgba(CHROME_ACTIVE_BG))
        })
        .child({
            let label = command.label().to_string();
            let mut arm = div()
                .id(SharedString::from(format!("creation-pill-{index}-arm")))
                .debug_selector({
                    let selector = format!("creation-pill-{index}-arm");
                    move || selector
                })
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .px_2()
                .py_1()
                .rounded_sm()
                .text_xs()
                .text_color(rgb(ink))
                .child(div().text_color(rgb(MUTED)).child(key))
                .child("⊕")
                .child(div().text_color(rgb(MUTED)).child(format!("{category}:")))
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(ink))
                        .child(label),
                );
            if arm_enabled {
                arm = arm
                    .cursor_pointer()
                    .hover(|style| style.bg(rgba(ROW_HOVER)).text_color(rgb(FG)))
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.activate_creation_slot(index, cx);
                    }));
            }
            arm
        })
        .child({
            let caret = div()
                .id(SharedString::from(format!("creation-pill-{index}-caret")))
                .debug_selector({
                    let selector = format!("creation-pill-{index}-caret");
                    move || selector
                })
                .px_1()
                .py_1()
                .rounded_sm()
                .text_xs()
                .text_color(rgb(MUTED))
                // `^` while this pill's flyout is open: the caret names the state of
                // the list it owns, not the direction of a drop-down.
                .child(if open { "^" } else { "▾" });
            if blocked {
                caret
            } else {
                let category = category.to_string();
                caret
                    .cursor_pointer()
                    .hover(|style| style.bg(rgba(ROW_HOVER)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _window, _cx| {
                            this.set_popover_anchor(event.position);
                        }),
                    )
                    .on_click(cx.listener(move |this, _event, _window, cx| {
                        this.open_creation_flyout(&category, cx);
                    }))
            }
        });
    container.into_any_element()
}

/// The creation flyout: a segmented category header over **one** category's tools.
///
/// The header is the strip's own categories, and a category with no registered tool is
/// not a tab at all. Showing one category at a time is what keeps the panel four or five
/// rows instead of the fifteen-tool column it used to be.
fn creation_flyout(
    creation: &[&CommandInfo],
    dock: &CreationDock,
    model: &ShellModel,
    anchor: Option<Point<Pixels>>,
    cx: &mut Context<Shell>,
) -> Option<AnyElement> {
    let tools: Vec<&CommandInfo> = creation
        .iter()
        .copied()
        .filter(|command| !command.dev_only)
        .collect();
    let categories = creation_categories(creation);
    if tools.is_empty() || categories.is_empty() {
        return None;
    }

    // The open tab: the dock's, else the armed tool's category, else the first.
    let active = dock
        .tab()
        .filter(|tab| categories.iter().any(|name| name.eq_ignore_ascii_case(tab)))
        .map(str::to_string)
        .or_else(|| {
            let armed = dock.active()?;
            let command = tools.iter().find(|command| command.id == armed)?;
            command.category().map(str::to_string)
        })
        .or_else(|| categories.first().map(|name| name.to_string()))?;

    let tabs: Vec<MenuTab> = categories
        .iter()
        .map(|name| MenuTab {
            id: (*name).to_string().into(),
            label: (*name).to_string().into(),
            active: name.eq_ignore_ascii_case(&active),
        })
        .collect();

    let mut rows = Vec::new();
    for command in &tools {
        if !command
            .category()
            .is_some_and(|name| name.eq_ignore_ascii_case(&active))
            || rows.len() >= CREATION_ROW_KEYS
        {
            continue;
        }
        let mut row = PopoverRow::of(command);
        // The list is **numbered**: `1`–`8` pick the tool into its pill without moving
        // the pointer. The number is a separate column from the gate's hint, so a row
        // can say both "not yet" and "this is 3".
        row.key = (rows.len() + 1).to_string();
        if !gate_satisfied(model, command)
            && let Some(gate) = command.topology_gate()
        {
            // Disabled in `INACTIVE_STRUCTURAL`, with the requirement as the row's
            // right-hand hint: the reason is visible before the click.
            row.enabled = false;
            row.accel = gate.requirement();
        }
        rows.push(row);
    }

    // Above the dock: the strip is at the bottom, so the panel grows upward from just
    // above the press (the header's menus grow downward by the same rule).
    let position = anchor
        .map(|pointer| point(pointer.x, pointer.y - px(34.)))
        .unwrap_or_else(|| point(px(0.), px(0.)));
    Some(
        deferred(
            anchored()
                .anchor(Anchor::BottomLeft)
                .position(position)
                .snap_to_window_with_margin(px(8.))
                .child(creation_popover(tabs, rows, cx)),
        )
        .with_priority(1)
        .into_any_element(),
    )
}

/// The flyout's panel: the reusable menu with a tab bar, wired back to the shell.
fn creation_popover(
    tabs: Vec<MenuTab>,
    rows: Vec<PopoverRow>,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    popover_with_tabs("create", Vec::new(), tabs, rows, Some("creation_pill"), cx)
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
        let icon_selector = id.clone();
        let glyph = command_icons::resolve(command)
            .and_then(|key| icon::named(&key, icon::ROW, if enabled { FG } else { MUTED }))
            .map(|glyph| {
                div()
                    .debug_selector(move || format!("dev-command-{icon_selector}-icon"))
                    .flex()
                    .items_center()
                    .child(glyph)
            });
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
            .when_some(glyph, |el, glyph| el.child(glyph))
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
/// The bug is an embedded SVG ([`icon::bug`], no font dependency), inked with the
/// accent while the window is open. The status bar is its home because Dev Tools is a
/// tool for the shell, not a modelling verb the viewport dock should carry.
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
                crate::types::GridProvider {
                    backend: "ferrocad".to_string(),
                    label: "FerroCAD (native)".to_string(),
                    active: true,
                    command: None,
                },
                crate::types::GridProvider {
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
                crate::types::GridProvider {
                    backend: "ferrocad".to_string(),
                    label: "FerroCAD (native)".to_string(),
                    active: true,
                    command: None,
                },
                crate::types::GridProvider {
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
            crate::types::GridProvider {
                backend: "ferrocad".to_string(),
                label: "FerroCAD (native)".to_string(),
                active: true,
                command: None,
            },
            crate::types::GridProvider {
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
                    crate::types::GridProvider {
                        backend: "ferrocad".to_string(),
                        label: "FerroCAD (native)".to_string(),
                        active: true,
                        command: Some("GridOverlay_Settings".to_string()),
                    },
                    crate::types::GridProvider {
                        backend: "coin".to_string(),
                        label: "Coin (WGSL)".to_string(),
                        active: false,
                        command: Some("GridOverlayCoin_Settings".to_string()),
                    },
                ],
            },
            ProviderFeature {
                feature: "gizmo".to_string(),
                providers: vec![crate::types::GridProvider {
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
    // supplied, the shape `crate::app::provider_features()` returns.
    fn gizmo_feature() -> ProviderFeature {
        ProviderFeature {
            feature: "gizmo".to_string(),
            providers: vec![
                crate::types::GridProvider {
                    backend: "ferrocad".to_string(),
                    label: "FerroCAD (native)".to_string(),
                    active: true,
                    command: Some("GizmoOverlay_Settings".to_string()),
                },
                crate::types::GridProvider {
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
            providers: vec![crate::types::GridProvider {
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
        // Before the interpreter starts, so the app script shares this binary's core.
        crate::register_test_core();
        let model = cx.update(|cx| {
            crate::app::install_backend(cx, Box::new(ferrocad_shell_python::engine()));
            Shell::boot(cx).expect("boot shell")
        });
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
        for selector in ["grid-icon", "axis-icon", "wp-icon", "gizmo-icon", "snap-icon"] {
            assert!(
                cx.debug_bounds(selector).is_some(),
                "{selector} is painted"
            );
        }
        let wp = cx.debug_bounds("wp-label").expect("the workplane control");
        let gizmo = cx.debug_bounds("gizmo-label").expect("the gizmo control");
        let select = cx.debug_bounds("select-label").expect("the select control");
        let snap = cx.debug_bounds("snap-label").expect("the snap control");
        assert!(
            wp.origin.x < gizmo.origin.x
                && gizmo.origin.x < select.origin.x
                && select.origin.x < snap.origin.x,
            "Select sits between Gizmo and Snap"
        );
        assert!(
            cx.debug_bounds("gizmo-state-dot").is_none(),
            "the icon ink replaces the state dot"
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

        // The topology filter is a selector: its caret opens the four filters, and the
        // current one carries the radio dot.
        click_at(&mut cx, "select-caret");
        draw(&mut cx);
        for selector in [
            "select-row-select:auto",
            "select-row-select:body",
            "select-row-select:face",
            "select-row-select:edge",
        ] {
            assert!(
                cx.debug_bounds(selector).is_some(),
                "{selector} is rendered"
            );
        }
        assert!(
            cx.debug_bounds("select-row-select:body-dot").is_some(),
            "Slot 1's Body filter is checked at rest"
        );
    }
}
