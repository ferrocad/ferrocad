//! Shell drawing helpers: overlays, panels, HUD placement and the tool strip.
//!
//! These are free functions over the state types in the parent module. The
//! `Render` impl stays in `shell.rs`; this file owns the pieces it composes.

use super::*;

/// The active-action surface: the active tool's label, point count, hint and a
/// **Cancel** / **Finish** pair, as one dielectric pill over the bottom of the canvas.
///
/// It is absolutely positioned in the same bottom-centre slot as the idle action dock
/// (see `chrome::action_dock`), so opening a tool never moves the canvas. The shell
/// swaps this for the dock while a session runs; the two are never stacked.
///
/// **Cancel** is neutral (no accent fill) and ends the session through
/// [`Shell::cancel_tool`]; **Finish** is the accent action and takes the same host path
/// as `Enter` ([`Shell::finish_active_tool`]), so the button and the key cannot drift.
pub(super) fn tool_strip(status: &ToolStatus, cx: &mut Context<Shell>) -> impl IntoElement {
    let pill = div()
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
        .child(div().text_color(rgb(FG)).child(status.label.clone()))
        .child(div().text_color(rgb(MUTED)).child(format!(
            "{} point{}",
            status.points,
            if status.points == 1 { "" } else { "s" }
        )))
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_color(rgb(MUTED))
                .child(status.hint.clone()),
        )
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
                .child("Cancel")
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.cancel_tool(cx);
                })),
        )
        // Finish is a real button, not a hint: a trackpad user can end the session
        // without finding `Enter`. It calls the same host path as the key.
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
                .child("Finish")
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.finish_active_tool(cx);
                })),
        );

    // The same bottom-centre overlay the dock uses: absolute, so it takes no space in
    // the viewport column and the canvas keeps its full rectangle.
    div()
        .absolute()
        .left(px(0.))
        .right(px(0.))
        .bottom(px(12.))
        .flex()
        .justify_center()
        .child(pill)
}

/// The window margin a floating HUD keeps from every edge.
pub(super) const HUD_MARGIN: f32 = 16.0;
/// The clearance below a floating HUD, so it never covers the bottom dock.
pub(super) const HUD_DOCK_CLEARANCE: f32 = 64.0;
/// Where the HUD first appears relative to its anchor: below-right of the gizmo.
pub(super) const HUD_OFFSET_X: f32 = 24.0;
pub(super) const HUD_OFFSET_Y: f32 = 88.0;

/// The inclusive range of HUD origins that keep the whole rect inside the viewport
/// safe area (16 px margins, 64 px above the bottom dock).
pub(super) fn hud_origin_range(viewport: Size<Pixels>, hud: Size<Pixels>) -> (f32, f32) {
    let max_x = (viewport.width.0 - hud.width.0 - HUD_MARGIN).max(HUD_MARGIN);
    let max_y = (viewport.height.0 - hud.height.0 - HUD_DOCK_CLEARANCE).max(HUD_MARGIN);
    (max_x, max_y)
}

/// Keep a HUD origin inside the viewport safe area.
pub(super) fn clamp_hud(origin: Point<Pixels>, viewport: Size<Pixels>, hud: Size<Pixels>) -> Point<Pixels> {
    let (max_x, max_y) = hud_origin_range(viewport, hud);
    point(
        px(origin.x.0.clamp(HUD_MARGIN, max_x)),
        px(origin.y.0.clamp(HUD_MARGIN, max_y)),
    )
}

/// Where a floating HUD appears: below-right of `anchor`, clamped into the viewport
/// safe area. `anchor` is the gizmo's visible origin (or the live pointer); the HUD
/// must never leave the canvas, even when the gizmo is dragged into a corner.
pub(super) fn place_hud(anchor: Point<Pixels>, viewport: Size<Pixels>, hud: Size<Pixels>) -> Point<Pixels> {
    let offset = point(px(anchor.x.0 + HUD_OFFSET_X), px(anchor.y.0 + HUD_OFFSET_Y));
    clamp_hud(offset, viewport, hud)
}

/// A command dialog (A3): a floating dielectric HUD, or a glass modal over a dimmed
/// shell when the spec keeps the default (`"presentation": "modal"`).
///
/// A floating spec has **no backdrop**: the canvas stays bright and the gizmo keeps
/// working, so typing a delta and dragging an arrow describe the same move. Only the
/// panel itself occludes the pointer. Its first field takes focus, `Tab` cycles the
/// fields, `Enter` accepts and `Esc` cancels; a text field's `Enter` submits through
/// its own `SubmitEvent`, wired to the same accept path.
pub(super) fn dialog_overlay(shell: &mut Shell, window: &Window, cx: &mut Context<Shell>) -> AnyElement {
    let Some(dialog) = shell.dialog.as_ref() else {
        return div().into_any_element();
    };
    let floating = dialog.spec.is_floating();
    let spec = dialog.spec.clone();
    let states = dialog.states.clone();
    let inputs = dialog.inputs.clone();
    let error = dialog.error.clone();
    let focus = shell.dialog_focus.clone();

    let mut fields = div().flex().gap_2();
    fields = if floating {
        fields.flex_row().items_center().flex_wrap()
    } else {
        fields.flex_col()
    };
    for (index, field) in spec.fields.iter().enumerate() {
        let control: AnyElement = match field.kind() {
            Kind::Bool => {
                let checked = matches!(states.get(index), Some(ValueState::Flag(true)));
                form::toggle(
                    SharedString::from(format!("dialog-bool-{index}")),
                    checked,
                    cx.listener(move |this, _event, _window, cx| this.toggle_flag(index, cx)),
                )
                .into_any_element()
            }
            Kind::Enum => {
                let choice = match states.get(index) {
                    Some(ValueState::Choice(choice)) => *choice,
                    _ => 0,
                };
                let value = field.values.get(choice).cloned().unwrap_or_default();
                let key = format!("dialog-enum-{index}");
                let open = shell
                    .row_popup
                    .as_ref()
                    .is_some_and(|popup| popup.key == key);
                let values = field.values.clone();
                let click_key = key.clone();
                let click_value = value.clone();
                div()
                    .w(px(if floating { 132. } else { 170. }))
                    .flex_shrink_0()
                    .child(form::select_box(
                        SharedString::from(key),
                        &value,
                        open,
                        cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                            this.open_choice_popup(
                                &click_key,
                                &click_value,
                                values.clone(),
                                Some(index),
                                event.position(),
                                window.window_handle().window_id(),
                                cx,
                            );
                        }),
                    ))
                    .into_any_element()
            }
            Kind::Unknown => div()
                .text_xs()
                .text_color(rgb(0xef4444))
                .child(format!("unsupported kind {}", field.kind))
                .into_any_element(),
            _ => match inputs.get(index).and_then(Option::as_ref) {
                Some(input) => div()
                    .w(px(if floating { 84. } else { 170. }))
                    .flex_shrink_0()
                    .child(input.clone())
                    .into_any_element(),
                None => div().into_any_element(),
            },
        };
        let label_ink = if floating { axis_token(field) } else { MUTED };
        let mut row = div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(label_ink))
                    .child(field.label().to_string()),
            )
            .child(control);
        if !floating {
            row = row.justify_between().gap_3();
        }
        fields = fields.child(row);
    }

    let accept = spec.accept_label().to_string();
    let cancel = spec.cancel_label().to_string();

    if floating {
        let viewport = window.viewport_size();
        // The last painted size is the best estimate before this frame paints; the
        // first frame uses a conservative default.
        let hud_size = shell
            .hud_rect
            .map(|rect| rect.size)
            .unwrap_or_else(|| size(px(600.0), px(72.0)));
        // Freeze while the user is working in the HUD: over its rect, or typing into
        // a field. Otherwise it follows the gizmo, clamped to the safe area. The
        // rect is the one painted last frame, so entering or leaving is compared
        // against what the pointer actually saw.
        let focused = inputs
            .iter()
            .flatten()
            .any(|input| input.read(cx).focus_handle(cx).is_focused(window));
        let hovered = shell
            .hud_rect
            .is_some_and(|rect| rect.contains(&window.mouse_position()));
        let follow = place_hud(shell.dialog_anchor(window, cx), viewport, hud_size);
        let origin = if hovered || focused {
            shell.hud_lock.unwrap_or(follow)
        } else {
            follow
        };
        // Resize or a new dialog may shrink the safe area under a frozen origin.
        let origin = clamp_hud(origin, viewport, hud_size);
        shell.hud_lock = Some(origin);
        let mut panel = div()
            .id("dialog-panel")
            .debug_selector(|| "dialog-panel".to_string())
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(rgba(WHEEL_GLASS))
            .border_1()
            .border_color(rgba(DOCK_BORDER))
            .occlude()
            .track_focus(&focus)
            .focusable()
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.cancel_dialog(cx),
                    "enter" => this.accept_dialog(cx),
                    "tab" => this.cycle_dialog_focus(event.keystroke.modifiers.shift, window, cx),
                    _ => {}
                }
            }))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(spec.title.clone()),
                    )
                    .child(fields)
                    .child(dialog_buttons(accept, cancel, true, cx)),
            );
        if let Some(error) = &error {
            panel = panel.child(
                div()
                    .text_xs()
                    .text_color(rgb(0xef4444))
                    .child(error.clone()),
            );
        }
        // The wrapper measures the painted panel, so the next frame can hit-test the
        // pointer against exactly where the HUD sits. Storing bounds does not notify:
        // it is read on the following frame.
        let weak = cx.weak_entity();
        let hud = div()
            .on_children_prepainted(move |bounds, _window, cx| {
                if let Some(rect) = bounds.first().copied() {
                    let _ = weak.update(cx, |shell, _cx| shell.hud_rect = Some(rect));
                }
            })
            .child(panel);
        return deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .position(origin)
                .child(hud),
        )
        .with_priority(2)
        .into_any_element();
    }

    let mut panel = div()
        .id("dialog-panel")
        .debug_selector(|| "dialog-panel".to_string())
        .w(px(380.))
        .flex()
        .flex_col()
        .gap_3()
        .p_4()
        .rounded_md()
        .bg(rgba(0x16181df2))
        .border_1()
        .border_color(rgba(0xffffff26))
        .track_focus(&focus)
        .focusable()
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
            match event.keystroke.key.as_str() {
                "escape" => this.cancel_dialog(cx),
                "enter" => this.accept_dialog(cx),
                _ => {}
            }
        }))
        .child(
            div()
                .text_sm()
                .text_color(rgb(FG))
                .child(spec.title.clone()),
        )
        .child(fields);
    if let Some(error) = &error {
        panel = panel.child(
            div()
                .text_xs()
                .text_color(rgb(0xef4444))
                .child(error.clone()),
        );
    }
    panel = panel.child(dialog_buttons(accept, cancel, false, cx));

    div()
        .id("dialog-backdrop")
        .debug_selector(|| "dialog-backdrop".to_string())
        .absolute()
        .left(px(0.))
        .top(px(0.))
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(rgba(0x0f1218cc))
        .occlude()
        .child(panel)
        .into_any_element()
}

/// The axis ink for a floating field: `dx`/`dy`/`dz` (or `x`/`y`/`z`) get their
/// tokens, everything else stays muted.
pub(super) fn axis_token(field: &crate::dialog::DialogField) -> u32 {
    match field.name.to_ascii_lowercase().as_str() {
        "dx" | "x" => AXIS_X,
        "dy" | "y" => AXIS_Y,
        "dz" | "z" => AXIS_Z,
        _ => MUTED,
    }
}

/// The accept/cancel pair. `compact` tightens the padding for the floating HUD.
pub(super) fn dialog_buttons(
    accept: String,
    cancel: String,
    compact: bool,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let pad = if compact { px(2.) } else { px(3.) };
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_end()
        .gap_2()
        .child(
            div()
                .id("dialog-cancel")
                .debug_selector(|| "dialog-cancel".to_string())
                .px(pad)
                .py_1()
                .rounded_sm()
                .bg(rgb(PANEL_ALT))
                .text_xs()
                .text_color(rgb(FG))
                .child(cancel)
                .on_click(cx.listener(|this, _event, _window, cx| this.cancel_dialog(cx))),
        )
        .child(
            div()
                .id("dialog-accept")
                .debug_selector(|| "dialog-accept".to_string())
                .px(pad)
                .py_1()
                .rounded_sm()
                .bg(rgb(ACCENT))
                .text_xs()
                .text_color(rgb(FG))
                .child(accept)
                .on_click(cx.listener(|this, _event, _window, cx| this.accept_dialog(cx))),
        )
}

/// The command palette (Ctrl+K): a focused search field over the filtered registry.
///
/// The field holds keyboard focus, so typing filters and `Esc` bubbles to the shell
/// root's [`Shell::handle_key`]. `Up`/`Down` are not editing keys, so they bubble to
/// this panel's key handler; `Enter` reaches the shell through the field's
/// `SubmitEvent` (see [`Shell::open_palette`]).
pub(super) fn palette_overlay(
    shell: &Shell,
    model: &ShellModel,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let Some(palette) = shell.palette.as_ref() else {
        return div().into_any_element();
    };
    let input = palette.input.clone();
    let query = palette.query.clone();
    let commands = palette_filtered(&model.commands, &query);
    // The cursor may trail the list while commands refresh; clamp it here and in the
    // accept path so the highlighted row is always the row Enter runs.
    let selected = palette.selected.min(commands.len().saturating_sub(1));

    let mut rows = div()
        .id("palette-rows")
        .flex()
        .flex_col()
        .gap_1()
        .min_h_0()
        .max_h(px(320.))
        .overflow_y_scroll();
    if commands.is_empty() {
        rows = rows.child(
            div()
                .debug_selector(|| "palette-empty".to_string())
                .px_2()
                .py_2()
                .text_xs()
                .text_color(rgb(MUTED))
                .child("No matching commands"),
        );
    }
    for (index, command) in commands.iter().enumerate() {
        let id = command.id.clone();
        let selector = id.clone();
        let enabled = command.enabled;
        let checked = command.checked;
        let is_selected = index == selected;
        let mut row = div()
            .id(SharedString::from(format!("palette-row-{id}")))
            .debug_selector(move || format!("palette-row-{selector}"))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .text_xs()
            .text_color(rgb(if enabled { FG } else { MUTED }))
            .when(is_selected, |el| el.bg(rgb(ACCENT)))
            .when(enabled, |el| {
                el.cursor_pointer().hover(|style| style.bg(rgb(PANEL_ALT)))
            })
            .child(command.label().to_string())
            .child(div().flex_1())
            .when(checked, |el| {
                el.child(div().text_color(rgb(MUTED)).child("\u{2713}"))
            })
            .child(
                div()
                    .text_color(rgb(MUTED))
                    .child(command.surface().as_str()),
            );
        if enabled {
            row = row.on_click(cx.listener(move |this, _event, _window, cx| {
                this.close_palette(cx);
                this.run_command(&id, cx);
            }));
        }
        rows = rows.child(row);
    }

    let panel = div()
        .id("palette")
        .debug_selector(|| "palette".to_string())
        .w(px(520.))
        .flex()
        .flex_col()
        .gap_2()
        .p_2()
        .rounded_md()
        .bg(rgba(0x16181df2))
        .border_1()
        .border_color(rgba(0xffffff26))
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| {
            match event.keystroke.key.as_str() {
                "up" => {
                    this.move_palette_selection(-1, cx);
                    cx.stop_propagation();
                }
                "down" => {
                    this.move_palette_selection(1, cx);
                    cx.stop_propagation();
                }
                _ => {}
            }
        }))
        .child(
            div()
                .debug_selector(|| "palette-input".to_string())
                .rounded_sm()
                .border_1()
                .border_color(rgba(0xffffff26))
                .child(input),
        )
        .child(rows)
        .child(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .px_1()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(format!("{} command(s)", commands.len()))
                .child("\u{2191}\u{2193} select \u{00b7} Enter run \u{00b7} Esc close"),
        );

    div()
        .id("palette-backdrop")
        .absolute()
        .left(px(0.))
        .top(px(0.))
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .pt(px(72.))
        .bg(rgba(0x0f1218b3))
        .occlude()
        .child(panel)
        .into_any_element()
}

/// The model inspector: documents and their objects.
///
/// The accordion supplies the section header, so the pane is only the scrollable
/// rows.
pub(crate) fn tree_panel(
    model: &ShellModel,
    scroll: &ScrollHandle,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let mut rows = div().flex().flex_col();

    for doc in &model.tree {
        rows = rows.child(
            div()
                .px_2()
                .py_1()
                .text_xs()
                .text_color(rgb(MUTED))
                .child(format!("▾ {} ({})", doc.label, doc.name)),
        );
        for obj in &doc.objects {
            let name = obj.name.clone();
            let mut row = div()
                .id(SharedString::from(format!("obj-{name}")))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_4()
                .py_1()
                .child(div().text_sm().text_color(rgb(FG)).child(obj.label.clone()))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(obj.type_id.clone()),
                );
            if model
                .selection
                .iter()
                .any(|item| same_node(&item.object, &name))
            {
                row = row.bg(rgb(ACCENT));
            }
            rows = rows.child(row.on_click(cx.listener(
                move |this, event: &gpui::ClickEvent, _window, cx| {
                    // Ctrl/Cmd (the secondary modifier) or Shift extends the selection;
                    // a plain click replaces it.
                    let modifiers = event.modifiers();
                    if modifiers.secondary() || modifiers.shift {
                        this.toggle_select(&name, cx);
                    } else {
                        this.select(&name, cx);
                    }
                },
            )));
        }
    }

    div()
        .flex()
        .flex_col()
        .w_full()
        .flex_1()
        .min_h_0()
        .bg(rgb(PANEL))
        .debug_selector(|| "tree-panel".to_string())
        .child(scroll_area("dev-model", scroll, rows))
}

/// The property editor: read-only rows plus an uncontrolled field for each
/// editable property.
///
/// The Dev Tools pane shows the current selection; the settings card calls
/// [`properties_rows_panel`] directly with its own node and rows.
pub(crate) fn properties_panel(
    shell: &mut Shell,
    model: &ShellModel,
    window: &Window,
    scroll: &ScrollHandle,
    cx: &mut Context<Shell>,
) -> impl IntoElement + use<> {
    properties_rows_panel(
        shell,
        model.selected.as_deref(),
        &model.properties,
        window,
        scroll,
        cx,
    )
}

/// The property rows for one node.
///
/// The rows come in as a parameter so a surface that does not own the selection
/// (the floating settings card) can show its own node.
pub(super) fn properties_rows_panel(
    shell: &mut Shell,
    selected: Option<&str>,
    properties: &[PropRow],
    window: &Window,
    scroll: &ScrollHandle,
    cx: &mut Context<Shell>,
) -> impl IntoElement + use<> {
    let mut rows = div().flex().flex_col();

    match selected {
        None => {
            rows = rows.child(
                div()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("No selection"),
            );
        }
        Some(name) => {
            rows = rows.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(FG))
                    .child(name.to_string()),
            );
            for row in properties {
                let value: AnyElement = if let Some(command) = row.command.clone() {
                    // A generated action row (the settings card's `Reset to edition
                    // defaults`). It runs a real command, not a property edit, so the
                    // request it returns (the restored appearance) is applied by
                    // `run_command` rather than dropped.
                    let label = if row.label.is_empty() {
                        row.name.clone()
                    } else {
                        row.label.clone()
                    };
                    let selector = format!("prop-command-{command}");
                    div()
                        .debug_selector(move || selector.clone())
                        .id(SharedString::from(format!("prop-command-{command}")))
                        .flex_shrink_0()
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .text_xs()
                        .text_color(rgb(FG))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(PANEL_ALT)))
                        .on_click(cx.listener(move |shell, _event, _window, cx| {
                            shell.run_command(&command, cx);
                            cx.notify();
                        }))
                        .child(label)
                        .into_any_element()
                } else if row.editable {
                    let key = format!("{name}.{}", row.name);
                    let selector = format!("prop-{key}");
                    let slider_range = if matches!(row.kind.as_str(), "float" | "int" | "quantity")
                    {
                        row.min.zip(row.max).filter(|(min, max)| max > min)
                    } else {
                        None
                    };
                    // A dimension without a declared range is not a position on a
                    // track: its handle is dragged directly and a scale appears.
                    let scrubbable = row.kind == "quantity" && slider_range.is_none();
                    let label = row_label(row);
                    let open = shell
                        .row_popup
                        .as_ref()
                        .is_some_and(|popup| popup.key == key);
                    if row.kind == KIND_COLOR {
                        let state = shell.ensure_prop_input(&key, &row.value, window, cx);
                        // A colour row keeps its text field (a typed hex still works)
                        // and gains a swatch that opens the palette overlay.
                        let popup_key = key.clone();
                        let popup_label = label.clone();
                        let popup_value = row.value.clone();
                        div()
                            .debug_selector(move || selector.clone())
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .w(px(180.))
                            .flex_shrink_0()
                            .child(color::swatch(
                                SharedString::from(format!("color-{key}")),
                                &row.value,
                                open,
                                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                    this.open_color_popup(
                                        &popup_key,
                                        &popup_label,
                                        &popup_value,
                                        event.position(),
                                        window.window_handle().window_id(),
                                        cx,
                                    );
                                }),
                            ))
                            .child(div().w(px(120.)).child(state))
                            .into_any_element()
                    } else if row.kind == KIND_BOOL {
                        // A boolean is a switch, not a text field: a click flips it
                        // and commits the opposite of what the row shows.
                        let commit_key = key.clone();
                        let checked = row_bool(&row.value);
                        div()
                            .debug_selector(move || selector.clone())
                            .w(px(150.))
                            .flex_shrink_0()
                            .child(form::toggle(
                                SharedString::from(format!("prop-bool-{key}")),
                                checked,
                                cx.listener(move |this, _event, _window, cx| {
                                    this.commit_property(
                                        &commit_key,
                                        if checked { "false" } else { "true" },
                                        cx,
                                    );
                                }),
                            ))
                            .into_any_element()
                    } else if row.kind == KIND_ENUM && !row.choices.is_empty() {
                        // An enumeration is a select box; its options open in the
                        // same anchored overlay as the palette.
                        let popup_key = key.clone();
                        let popup_value = row.value.clone();
                        let popup_choices = row.choices.clone();
                        div()
                            .debug_selector(move || selector.clone())
                            .w(px(150.))
                            .flex_shrink_0()
                            .child(form::select_box(
                                SharedString::from(format!("prop-enum-{key}")),
                                &row.value,
                                open,
                                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                    this.open_choice_popup(
                                        &popup_key,
                                        &popup_value,
                                        popup_choices.clone(),
                                        None,
                                        event.position(),
                                        window.window_handle().window_id(),
                                        cx,
                                    );
                                }),
                            ))
                            .into_any_element()
                    } else if let Some((min, max)) = slider_range {
                        let state = shell.ensure_prop_input(&key, &row.value, window, cx);
                        // A row with a declared range gets the mini track that opens
                        // the overlay; typing an exact value still works.
                        let popup_key = key.clone();
                        let popup_label = label.clone();
                        let popup_value = row.value.clone();
                        let popup_unit = row_unit(row);
                        let current = row_number(&row.value).unwrap_or(min);
                        div()
                            .debug_selector(move || selector.clone())
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .w(px(180.))
                            .flex_shrink_0()
                            .child(div().w(px(112.)).child(state))
                            .child(slider::trigger(
                                SharedString::from(format!("slider-{key}")),
                                slider::fraction(current, min, max),
                                cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                    this.open_slider_popup(
                                        &popup_key,
                                        &popup_label,
                                        &popup_value,
                                        min,
                                        max,
                                        &popup_unit,
                                        event.position(),
                                        window.window_handle().window_id(),
                                        cx,
                                    );
                                }),
                            ))
                            .into_any_element()
                    } else if scrubbable {
                        let state = shell.ensure_prop_input(&key, &row.value, window, cx);
                        // The handle is dragged directly. The press starts the gesture
                        // at the pointer and the window root forwards the moves, so the
                        // scale appears while the button is held (no second click); the
                        // release commits.
                        let active = shell.scrub.as_ref().is_some_and(|drag| drag.key == key);
                        let start = row_number(&row.value).unwrap_or(0.0);
                        let drag_key = key.clone();
                        let drag_label = label.clone();
                        let drag_unit = row_unit(row);
                        let handle_key = format!("scrub-{key}");
                        div()
                            .debug_selector(move || selector.clone())
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .w(px(180.))
                            .flex_shrink_0()
                            .child(div().w(px(112.)).child(state))
                            .child(
                                div()
                                    .id(SharedString::from(handle_key.clone()))
                                    .debug_selector(move || handle_key.clone())
                                    .w(px(22.))
                                    .h(px(18.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_sm()
                                    .cursor_ns_resize()
                                    .hover(|style| style.bg(rgb(PANEL)))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(
                                            move |this, event: &MouseDownEvent, window, cx| {
                                                this.start_scrub(
                                                    &drag_key,
                                                    &drag_label,
                                                    &drag_unit,
                                                    start,
                                                    event.position,
                                                    window.window_handle().window_id(),
                                                    cx,
                                                );
                                                cx.stop_propagation();
                                            },
                                        ),
                                    )
                                    .child(scrub::handle(
                                        SharedString::from(format!("scrub-{key}")),
                                        active,
                                    )),
                            )
                            .into_any_element()
                    } else {
                        let state = shell.ensure_prop_input(&key, &row.value, window, cx);
                        div()
                            .debug_selector(move || selector.clone())
                            .w(px(150.))
                            .flex_shrink_0()
                            .child(state)
                            .into_any_element()
                    }
                } else {
                    let value = if row.status.is_empty() {
                        row.value.clone()
                    } else {
                        format!("{}  [{}]", row.value, row.status)
                    };
                    div()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(rgb(MUTED))
                        .child(value)
                        .into_any_element()
                };

                rows = rows.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_1()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .text_xs()
                                        .text_color(rgb(FG))
                                        .child(row.name.clone()),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .text_color(rgb(MUTED))
                                        .child(row.type_id.clone()),
                                ),
                        )
                        .child(value),
                );
            }
        }
    }

    let panel = div()
        .flex()
        .flex_col()
        .w_full()
        .flex_1()
        .min_h_0()
        .bg(rgb(PANEL))
        .debug_selector(|| "properties-panel".to_string());
    panel.child(scroll_area("dev-properties", scroll, rows))
}

/// The display label for a property row: its `label` when the schema supplied
/// one, otherwise its name.
pub(super) fn row_label(row: &crate::python::PropRow) -> String {
    if row.label.is_empty() {
        row.name.clone()
    } else {
        row.label.clone()
    }
}

/// The number a property row displays (`80`, `0.3`, `10 mm` -> 10).
pub(super) fn row_number(value: &str) -> Option<f64> {
    value.split_whitespace().next()?.parse().ok()
}

/// The unit a quantity row's overlay readout and commit carry.
///
/// A schema row declares `unit`; a document property's row has none, so the
/// unit is the suffix of its displayed value (`10 mm` -> `mm`). Without it the
/// dial or slider commit would assign a dimensionless quantity and strip it.
pub(super) fn row_unit(row: &PropRow) -> String {
    if let Some(unit) = row.unit.as_deref().filter(|unit| !unit.is_empty()) {
        return unit.to_string();
    }
    if row.kind != "quantity" {
        return String::new();
    }
    row.value
        .trim()
        .trim_start_matches(|c: char| {
            c.is_ascii_digit() || matches!(c, '+' | '-' | '.' | 'e' | 'E')
        })
        .trim()
        .to_string()
}

/// Whether a property row's display value reads as `true` (`True`, `1`, `yes`,
/// `on` — the Python and parameter stores spell it differently).
pub(super) fn row_bool(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "true" | "1" | "yes" | "on"
    )
}

/// The pointer position as a 0..1 fraction of the slider track.
pub(super) fn track_fraction(popup: &RowPopup, x: Pixels) -> f32 {
    let Some(track) = popup.track else {
        return 0.0;
    };
    let width = track.size.width.0.max(1.0);
    ((x.0 - track.left().0) / width).clamp(0.0, 1.0)
}

/// The anchored overlay a property row opened: the colour palette, the range
/// slider or an enumeration's dropdown. (The scrub's scale is a separate overlay
/// driven by the held handle.)
///
/// It floats below the pointer, so it covers the pane instead of pushing its rows
/// down, and it is `deferred`, so it is not clipped by the scrolling pane. Only the
/// window that opened it renders it: the same property rows are built for the
/// settings card and for the Dev Tools window.
pub(super) fn row_popup_overlay(
    shell: &mut Shell,
    window: &Window,
    cx: &mut Context<Shell>,
) -> Option<AnyElement> {
    let popup = shell.row_popup.as_ref()?;
    if popup.window != window.window_handle().window_id() {
        return None;
    }
    let key = popup.key.clone();
    let label = popup.label.clone();
    let value = popup.value.clone();
    let anchor = popup.anchor;
    let kind = popup.kind;
    let range = popup.range;
    let unit = popup.unit.clone();
    let choices = popup.choices.clone();
    let dialog_field = popup.dialog_field;
    let draft = popup.draft;

    let close = {
        let weak = cx.weak_entity();
        move |_event: &MouseDownEvent, _window: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |shell, cx| shell.close_row_popup(cx));
        }
    };

    let body: AnyElement = match kind {
        RowPopupKind::Color => {
            let weak = cx.weak_entity();
            let pick_key = key.clone();
            color::palette(
                SharedString::from(format!("color-{key}")),
                &value,
                move |hex, _event, _window, cx| {
                    let _ = weak.update(cx, |shell, cx| {
                        shell.commit_property(&pick_key, hex, cx);
                    });
                },
            )
            .into_any_element()
        }
        RowPopupKind::Choices => {
            let weak = cx.weak_entity();
            let pick_key = key.clone();
            form::choice_list(
                SharedString::from(format!("choose-{key}")),
                &choices,
                &value,
                move |choice, _event, _window, cx| {
                    let _ = weak.update(cx, |shell, cx| match dialog_field {
                        Some(index) => shell.set_dialog_choice(index, choice, cx),
                        None => shell.commit_property(&pick_key, choice, cx),
                    });
                },
            )
            .into_any_element()
        }
        RowPopupKind::Slider => {
            let Some(range) = range else {
                return None;
            };
            let current = draft.unwrap_or_else(|| row_number(&value).unwrap_or(range.min));
            let position = slider::fraction(current, range.min, range.max);
            let readout = if unit.is_empty() {
                slider::format_value(current)
            } else {
                format!("{} {unit}", slider::format_value(current))
            };

            let track_selector = format!("slider-{key}-track");
            let marker = canvas(
                move |_bounds, _window, _cx| {},
                move |bounds, (), window, _cx| {
                    let height = px(6.);
                    let track = Bounds::new(
                        point(bounds.left(), bounds.center().y - height / 2.0),
                        size(bounds.size.width, height),
                    );
                    window.paint_quad(fill_track(track));
                    let filled = bounds.size.width.0 * position;
                    if filled > 0.0 {
                        window.paint_quad(fill_filled(Bounds::new(
                            point(bounds.left(), bounds.center().y - height / 2.0),
                            size(px(filled), height),
                        )));
                    }
                    let thumb_x = bounds.left().0 + position * (bounds.size.width.0 - 12.0).max(0.0);
                    window.paint_quad(fill_thumb(Bounds::new(
                        point(px(thumb_x), bounds.center().y - px(6.)),
                        size(px(12.), px(12.)),
                    )));
                },
            )
            .size_full();

            let down = {
                let weak = cx.weak_entity();
                let drag_key = key.clone();
                move |event: &MouseDownEvent, _window: &mut Window, cx: &mut App| {
                    let x = event.position.x;
                    let _ = weak.update(cx, |shell, cx| {
                        if let Some(popup) = shell.row_popup.as_mut() {
                            if popup.key == drag_key {
                                if let Some(range) = popup.range {
                                    popup.dragging = true;
                                    popup.draft = Some(slider::value_at(
                                        track_fraction(popup, x),
                                        range.min,
                                        range.max,
                                    ));
                                }
                            }
                        }
                        cx.notify();
                    });
                    cx.stop_propagation();
                }
            };
            let moved = {
                let weak = cx.weak_entity();
                let drag_key = key.clone();
                move |event: &MouseMoveEvent, _window: &mut Window, cx: &mut App| {
                    if !event.dragging() {
                        return;
                    }
                    let x = event.position.x;
                    let _ = weak.update(cx, |shell, cx| {
                        if let Some(popup) = shell.row_popup.as_mut() {
                            if popup.key == drag_key && popup.dragging {
                                if let Some(range) = popup.range {
                                    popup.draft = Some(slider::value_at(
                                        track_fraction(popup, x),
                                        range.min,
                                        range.max,
                                    ));
                                    cx.notify();
                                }
                            }
                        }
                    });
                }
            };
            let commit = {
                let weak = cx.weak_entity();
                let drag_key = key.clone();
                move |_event: &MouseUpEvent, _window: &mut Window, cx: &mut App| {
                    let _ = weak.update(cx, |shell, cx| {
                        if shell.row_popup.as_ref().is_some_and(|p| p.key == drag_key) {
                            if let Some(popup) = shell.row_popup.take() {
                                if let Some(draft) = popup.draft {
                                    let text = if popup.unit.is_empty() {
                                        slider::format_value(draft)
                                    } else {
                                        // A document quantity has no schema `unit`:
                                        // its unit comes from the displayed value.
                                        // Sending a bare number would assign a
                                        // dimensionless quantity and strip it.
                                        format!("{} {}", slider::format_value(draft), popup.unit)
                                    };
                                    shell.commit_property(&popup.key, &text, cx);
                                }
                            }
                            cx.notify();
                        }
                    });
                }
            };
            let track = div()
                // The bounds listener has to sit on the plain `Div` (it is not on
                // `Stateful`), so the wrapper records its child's track rect.
                .on_children_prepainted({
                    let weak = cx.weak_entity();
                    let bounds_key = key.clone();
                    move |bounds, _window, cx| {
                        if let Some(rect) = bounds.first().copied() {
                            let _ = weak.update(cx, |shell, _cx| {
                                if let Some(popup) = shell.row_popup.as_mut() {
                                    if popup.key == bounds_key {
                                        popup.track = Some(rect);
                                    }
                                }
                            });
                        }
                    }
                })
                .child(
                    div()
                        .id(SharedString::from(track_selector.clone()))
                        .debug_selector(move || track_selector.clone())
                        .relative()
                        .h(px(20.))
                        .w_full()
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, down)
                        .on_mouse_move(moved)
                        .on_mouse_up(MouseButton::Left, commit.clone())
                        .on_mouse_up_out(MouseButton::Left, commit)
                        .child(marker),
                );

            let ends = div()
                .flex()
                .flex_row()
                .justify_between()
                .text_color(rgb(MUTED))
                .child(slider::format_value(range.min))
                .child(slider::format_value(range.max));

            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .gap_2()
                        .text_color(rgb(FG))
                        .child(label)
                        .child(div().text_color(rgb(MUTED)).child(readout)),
                )
                .child(track)
                .child(ends)
                .into_any_element()
        }
    };

    let panel = div()
        .id("row-popup")
        .debug_selector(|| "row-popup".to_string())
        .w(px(232.))
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded_md()
        .bg(rgba(WHEEL_GLASS))
        .border_1()
        .border_color(rgba(DOCK_BORDER))
        .occlude()
        .text_xs()
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.stop_propagation()
        })
        .on_mouse_down_out(close)
        .child(body);

    Some(
        deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .position(point(anchor.x, anchor.y + px(6.)))
                .snap_to_window_with_margin(px(8.))
                .child(panel),
        )
        // Above the settings card (priority 4): the palette, slider and dial are
        // opened *from* the card and must paint over it.
        .with_priority(5)
        .into_any_element(),
    )
}

/// The scale shown while a property row's handle is being dragged.
///
/// It is anchored beside the press (so it does not chase the pointer) and deferred,
/// so it paints over the pane and the settings card. Only the window that owns the
/// gesture renders it.
pub(super) fn scrub_overlay(shell: &Shell, window: &Window) -> Option<AnyElement> {
    let drag = shell.scrub.as_ref()?;
    if drag.window != window.window_handle().window_id() {
        return None;
    }
    // Place the scale beside the press, never over it: the handle is 18 px and the
    // Dev Tools window is narrow enough that a right-side panel would be snapped
    // back across the pointer. Once the panel covers the press, the pane stops
    // seeing the drag.
    let viewport = window.viewport_size();
    let gap = px(24.);
    let width = px(scrub::PANEL_WIDTH);
    let mut left = drag.anchor.x + gap;
    if left + width + px(8.) > viewport.width {
        left = drag.anchor.x - gap - width;
    }
    let left = left.max(px(8.));
    let panel = scrub::scale("property-scrub", &drag.label, drag.scrub, &drag.unit);
    Some(
        deferred(
            anchored()
                .anchor(Anchor::TopLeft)
                .position(point(left, drag.anchor.y - px(90.)))
                .snap_to_window_with_margin(px(8.))
                .child(panel),
        )
        // Above the row overlay (5): the scale is the active gesture's feedback.
        .with_priority(6)
        .into_any_element(),
    )
}

/// The slider track's painted background, fill and thumb.
pub(super) fn fill_track(bounds: Bounds<Pixels>) -> gpui::PaintQuad {
    gpui::fill(bounds, rgb(PANEL_ALT)).corner_radii(px(3.))
}

pub(super) fn fill_filled(bounds: Bounds<Pixels>) -> gpui::PaintQuad {
    gpui::fill(bounds, rgb(ACCENT)).corner_radii(px(3.))
}

pub(super) fn fill_thumb(bounds: Bounds<Pixels>) -> gpui::PaintQuad {
    gpui::fill(bounds, rgb(0x48e5ff)).corner_radii(px(6.))
}

/// The title the settings card uses for a synthetic node (`Settings/Grid` ->
/// `Grid settings`).
pub(super) fn settings_title(name: &str) -> String {
    let feature = name.strip_prefix("Settings/").unwrap_or(name);
    let mut chars = feature.chars();
    let title = match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    };
    format!("{title} settings")
}

/// The modeless settings card (A3/T5): a draggable `380 px` panel over the canvas.
///
/// It has no backdrop, renders its own `Settings/<Feature>` rows (not the current
/// selection's), and is dismissed with its `✕` or `Esc` — never by a selection
/// change. The position is window coordinates so it survives a drag across the
/// canvas.
pub(super) fn settings_card_view(
    shell: &mut Shell,
    window: &Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let (position, node, rows) = match shell.settings_card.as_ref() {
        Some(card) => (card.position, card.node.clone(), card.rows.clone()),
        None => (point(px(220.), px(96.)), String::new(), Vec::new()),
    };
    let title = settings_title(&node);

    let header = div()
        .id("settings-card-header")
        .debug_selector(|| "settings-card-header".to_string())
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .px_2()
        .py_1()
        .bg(rgb(PANEL_ALT))
        .text_xs()
        .text_color(rgb(FG))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, cx.listener(|this, event: &MouseDownEvent, _window, cx| {
            this.begin_settings_drag(event.position, cx);
        }))
        .child(title)
        .child(
            div()
                .id("settings-card-close")
                .debug_selector(|| "settings-card-close".to_string())
                .px_1()
                .rounded_sm()
                .text_color(rgb(MUTED))
                .hover(|style| style.bg(rgb(PANEL)))
                .child("✕")
                .on_mouse_down(MouseButton::Left, |_event, _window, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _event, _window, cx| {
                    this.close_settings_card(cx);
                })),
        );

    let panel = {
        let scroll = shell.dev_scroll.settings.clone();
        properties_rows_panel(shell, Some(&node), &rows, window, &scroll, cx)
    };
    let card = div()
        .id("settings-card")
        .debug_selector(|| "settings-card".to_string())
        // The card floats over the canvas: own the hitbox so a press/drag inside it
        // cannot orbit or pick the viewport underneath.
        .occlude()
        .flex()
        .flex_col()
        .w(px(380.))
        .h(px(420.))
        .rounded_md()
        .bg(rgba(0x16181df2))
        .border_1()
        .border_color(rgba(0xffffff26))
        .overflow_hidden()
        // `occlude` blocks the shell root's pointer moves while the drag is over the
        // card, so the card follows the drag itself.
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
            this.drag_settings_card(event.position, event.pressed_button, cx);
            this.drag_scrub(event.position, event.pressed_button, cx);
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| {
                this.end_settings_drag(cx);
                this.end_scrub(cx);
            }),
        )
        // A press elsewhere ends a scrub whose release was lost (for example when
        // the pointer left the window). The handle stops propagation, so its own
        // press reaches `start_scrub` and not this.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| {
                this.end_scrub(cx);
            }),
        )
        .child(header)
        .child(
            // A **column** wrapper: as a row, the panel's height was its content
            // height, so the rows were laid out past the card and clipped with
            // nothing to scroll. In a column, `flex_1` constrains the panel to the
            // card's content height and the scroll area gets a real viewport.
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(panel),
        );

    deferred(
        anchored()
            .anchor(Anchor::TopLeft)
            .position(position)
            .snap_to_window_with_margin(px(8.))
            .child(card),
    )
    // Above the floating HUD and the wheel: the card is what the user is working
    // in, and it must not be covered by a transient overlay.
    .with_priority(4)
}

/// The globally bindable accelerators for `commands`.
///
/// A shortcut is bound only when it is a safe global chord; a bare letter would
/// shadow typing in the console and the property editor (see [`Accelerator`]).
pub(super) fn global_shortcuts(commands: &[CommandInfo]) -> Vec<(Accelerator, String)> {
    commands
        .iter()
        .filter_map(|command| {
            let accelerator = Accelerator::parse(&command.accel)?;
            accelerator
                .is_global()
                .then(|| (accelerator, command.id.clone()))
        })
        .collect()
}

/// The status line's pass-through text: `"clicked through: navicube → Slab"`.
///
/// Empty when no overlay is under the pointer, so the segment disappears rather
/// than saying "nothing".
pub(super) fn hover_status(report: &HoverReport, target: Option<&str>) -> String {
    let Some(through) = report.through else {
        return String::new();
    };
    match target {
        Some(name) => format!("clicked through: {through} → {name}"),
        None => format!("clicked through: {through}"),
    }
}

/// A console action button.
pub(super) fn console_button(
    id: &str,
    label: &str,
    code: &str,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let code = code.to_string();
    div()
        .id(SharedString::from(id.to_string()))
        .px_2()
        .py_1()
        .rounded_sm()
        .bg(rgb(PANEL_ALT))
        .text_xs()
        .text_color(rgb(FG))
        .child(label.to_string())
        .on_click(cx.listener(move |this, _ev, _win, cx| {
            this.run(&code, cx);
        }))
}

/// The Python console: one text area (scrollback plus prompt) and runnable
/// snippets.
pub(crate) fn console_panel(
    console: &Entity<TextAreaState>,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let actions = div()
        .flex()
        .flex_row()
        .gap_2()
        .px_2()
        .py_1()
        .child(console_button("console-version", "version()", "FreeCAD.Version()", cx))
        .child(console_button("console-objects", "objects", "len(doc.Objects)", cx))
        .child(console_button("console-recompute", "recompute()", "doc.recompute()", cx));

    div()
        .flex()
        .flex_col()
        .h_full()
        .min_h_0()
        .bg(rgb(PANEL))
        .debug_selector(|| "console-panel".to_string())
        .child(console.clone())
        .child(actions)
}

/// Section indices in the Dev Tools accordion, in render order.
///
/// The body list is built in this order; the tests name indices through these
/// constants, which is why the module allows dead code in a non-test build.
#[allow(dead_code)]
pub(super) mod dev_section {
    pub const PROVIDERS: usize = 0;
    pub const GPU: usize = 1;
    pub const COMMANDS: usize = 2;
    pub const MODEL: usize = 3;
    pub const PROPERTIES: usize = 4;
    pub const CONSOLE: usize = 5;
}

/// The Dev Tools window's body: the whole command surface, the inspector, the property
/// editor and the console in one column (P0.5).
///
/// The column cannot hold every pane at once, so the panes are an accordion
/// ([`ferrocad_widgets::accordion`]): each keeps a header, and the layout gives the
/// remaining height to the open one. Whichever pane is open survives closing and
/// reopening the window (`Shell::dev_accordion`).
///
/// It is built from the shell's `Context`, so every row runs against the same `Shell`
/// entity the main window renders. The viewport is deliberately **not** here: gpui can
/// render an entity in two windows, but the 3D viewport must only be drawn once.
pub(crate) fn dev_tools_body(
    shell: &mut Shell,
    focus: &FocusHandle,
    window: &Window,
    cx: &mut Context<Shell>,
) -> AnyElement {
    let model = shell.snapshot();
    // The adapter is read from the viewport entity once per frame; the list itself
    // is cached when the window opens.
    let in_use = shell.in_use_adapter(cx);
    // The commands the losing provider owns: listed, but not runnable (P0 provider UX).
    let inactive = crate::chrome::inactive_provider_commands(shell.provider_features());
    // Each pane keeps its scrollbar handle on the shell; cloning one is an `Rc` bump,
    // and it keeps the mutable `shell` borrow out of the argument lists below.
    let providers_scroll = shell.dev_scroll.providers.clone();
    let gpu_scroll = shell.dev_scroll.gpu.clone();
    let commands_scroll = shell.dev_scroll.commands.clone();
    let model_scroll = shell.dev_scroll.model.clone();
    let properties_scroll = shell.dev_scroll.properties.clone();

    // Preferred heights are the old fixed pane sizes; they are only used to decide
    // whether the whole stack fits (it does not in the default window).
    let gpu = shell.gpu_setting().to_string();
    let command_count = model.commands.len();
    let sections = vec![
        AccordionSection::new("providers", "Providers", px(160.)),
        AccordionSection::new("gpu", "GPU adapter", px(180.)).with_summary(gpu),
        AccordionSection::new("commands", "Commands", px(260.))
            .with_summary(format!("{command_count}")),
        AccordionSection::new("model", "Model", px(200.)),
        AccordionSection::new("properties", "Properties", px(240.)),
        AccordionSection::new("console", "Python console", px(160.)),
    ];
    let bodies: Vec<AnyElement> = vec![
        crate::chrome::providers_panel(shell.provider_features(), &providers_scroll, cx)
            .into_any_element(),
        crate::chrome::gpu_panel(
            shell.gpu_setting(),
            shell.gpu_adapters(),
            in_use.as_ref(),
            &gpu_scroll,
            cx,
        )
        .into_any_element(),
        crate::chrome::command_list_panel(&model, &inactive, &commands_scroll, cx)
            .into_any_element(),
        tree_panel(&model, &model_scroll, cx).into_any_element(),
        properties_panel(shell, &model, window, &properties_scroll, cx).into_any_element(),
        console_panel(&shell.console, cx).into_any_element(),
    ];
    let available = ferrocad_widgets::window_chrome::content_height(window);
    let weak = cx.weak_entity();
    let on_toggle =
        move |index: usize, _event: &gpui::ClickEvent, _window: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |shell, cx| {
                shell.dev_accordion.toggle(index);
                cx.notify();
            });
        };

    div()
        .id("dev-tools-root")
        .flex()
        .flex_col()
        .size_full()
        .bg(rgb(BG))
        .text_color(rgb(FG))
        .debug_selector(|| "dev-tools".to_string())
        .track_focus(focus)
        .focusable()
        // Commands and shortcuts run from either window. `Esc` also reaches the modal
        // and tool paths, which stay on the shell root.
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
            if this.handle_key(event, true, window, cx) {
                cx.stop_propagation();
            }
        }))
        // A scrub's moves leave the 18 px handle immediately; the window root sees
        // them all, so the scale keeps following the pointer.
        .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
            this.drag_scrub(event.position, event.pressed_button, cx);
        }))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| {
                this.end_scrub(cx);
            }),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| {
                this.end_scrub(cx);
            }),
        )
        .child(accordion(
            "dev",
            sections,
            bodies,
            shell.dev_accordion.clone(),
            available,
            on_toggle,
        ))
        // A row overlay (the colour palette, the slider or an enum dropdown) opened
        // from this window's property pane. It is a window-root child so it does not
        // depend on which pane is open; `row_popup_overlay` ignores other windows.
        .when_some(row_popup_overlay(shell, window, cx), |root, overlay| {
            root.child(overlay)
        })
        // The scale of a handle being dragged in this window's property pane.
        .when_some(scrub_overlay(shell, window), |root, overlay| {
            root.child(overlay)
        })
        .into_any_element()
}
