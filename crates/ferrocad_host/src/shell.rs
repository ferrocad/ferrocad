//! The app-shell view (S1).
//!
//! One window with a title bar, a model inspector, a viewport placeholder, a
//! property editor and a Python console. The layout and the interpreter wiring
//! are the S1 deliverable: the tree, editor and console are already driven by
//! the live document through `python/ferrocad_shell`; richer widgets, commands
//! and menus come in later slices (see `docs/app-shell-vision.md`).

use std::sync::{Arc, Mutex};

use gpui::{
    Bounds, Context, CursorStyle, Decorations, HitboxBehavior, IntoElement, MouseButton, Pixels,
    Point, Render, ResizeEdge, SharedString, Size, Window, canvas, div, point, prelude::*, px, rgb,
};

use crate::python::{self, Bootstrap, DocumentNode, PropRow};

const BG: u32 = 0x101216;
const PANEL: u32 = 0x16181d;
const PANEL_ALT: u32 = 0x1d2026;
const FG: u32 = 0xd8dbe0;
const MUTED: u32 = 0x8b919a;
const ACCENT: u32 = 0x2f4a6d;

/// Everything the window renders, kept in an `Arc<Mutex<_>>` so click handlers
/// can mutate it and tests can inspect it.
#[derive(Debug, Default, Clone)]
pub struct ShellModel {
    pub title: String,
    pub version: String,
    pub backend: String,
    pub tree: Vec<DocumentNode>,
    pub selected: Option<String>,
    pub properties: Vec<PropRow>,
    pub console: Vec<String>,
    pub status: String,
}

pub struct Shell {
    model: Arc<Mutex<ShellModel>>,
}

impl Shell {
    /// Boot the interpreter, create the sample document, and assemble the shell.
    pub fn boot() -> Result<Self, String> {
        let boot: Bootstrap = python::bootstrap()?;
        let tree = python::model_tree().unwrap_or_default();
        let mut console = vec![format!(
            "FerroCAD shell · FreeCAD {} · backend {}",
            boot.version, boot.backend
        )];
        match python::hello() {
            Ok(line) => console.push(line),
            Err(e) => console.push(format!("python: {e}")),
        }
        let objects: usize = tree.iter().map(|d| d.objects.len()).sum();
        let status = format!("{objects} object(s) · {}", boot.document);
        Ok(Self {
            model: Arc::new(Mutex::new(ShellModel {
                title: format!("FerroCAD — {}", boot.document),
                version: boot.version,
                backend: boot.backend,
                tree,
                selected: None,
                properties: Vec::new(),
                console,
                status,
            })),
        })
    }

    /// Build a shell around an existing model (the window and tests use this).
    pub fn from_model(model: Arc<Mutex<ShellModel>>) -> Self {
        Self { model }
    }

    pub fn model(&self) -> Arc<Mutex<ShellModel>> {
        Arc::clone(&self.model)
    }

    /// Select an object; the property editor is filled from the live model.
    fn select(&mut self, name: &str) {
        let properties = python::properties(name).unwrap_or_default();
        let mut m = self.model.lock().unwrap();
        m.selected = Some(name.to_string());
        m.properties = properties;
        m.status = format!("selected {name}");
    }

    /// Run a console snippet and refresh the tree (the model may have changed).
    fn run(&mut self, code: &str) {
        let result = python::evaluate(code);
        let tree = python::model_tree().unwrap_or_default();
        let mut m = self.model.lock().unwrap();
        m.console.push(format!(">>> {code}"));
        match result {
            Ok(r) => {
                if !r.output.is_empty() {
                    m.console.push(r.output);
                }
                m.status = if r.ok {
                    "ok".to_string()
                } else {
                    "console error".to_string()
                };
            }
            Err(e) => m.console.push(format!("error: {e}")),
        }
        m.tree = tree;
    }

    fn snapshot(&self) -> ShellModel {
        self.model.lock().unwrap().clone()
    }
}

/// A slim panel header bar.
fn panel_header(text: &str) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .px_2()
        .py_1()
        .bg(rgb(PANEL_ALT))
        .text_xs()
        .text_color(rgb(MUTED))
        .child(text.to_string())
}

/// The model inspector: documents and their objects.
fn tree_panel(model: &ShellModel, cx: &mut Context<Shell>) -> impl IntoElement {
    let mut panel = div()
        .flex()
        .flex_col()
        .w(px(260.))
        .h_full()
        .bg(rgb(PANEL))
        .child(panel_header("Model"));

    for doc in &model.tree {
        panel = panel.child(
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
            if model.selected.as_deref() == Some(name.as_str()) {
                row = row.bg(rgb(ACCENT));
            }
            panel = panel.child(row.on_click(cx.listener(move |this, _ev, _win, cx| {
                this.select(&name);
                cx.notify();
            })));
        }
    }
    panel
}

/// The property editor (S1: read-only rows for the selected object).
fn properties_panel(model: &ShellModel) -> impl IntoElement {
    let mut panel = div()
        .flex()
        .flex_col()
        .w(px(320.))
        .h_full()
        .bg(rgb(PANEL))
        .child(panel_header("Properties"));

    match &model.selected {
        None => {
            panel = panel.child(
                div()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("No selection"),
            );
        }
        Some(name) => {
            panel = panel.child(
                div()
                    .px_2()
                    .py_1()
                    .text_xs()
                    .text_color(rgb(FG))
                    .child(name.clone()),
            );
            for row in &model.properties {
                let value = if row.status.is_empty() {
                    row.value.clone()
                } else {
                    format!("{}  [{}]", row.value, row.status)
                };
                panel = panel.child(
                    div()
                        .flex()
                        .flex_row()
                        .justify_between()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .min_w_0()
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .gap_1()
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
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_xs()
                                .text_color(rgb(MUTED))
                                .child(value),
                        ),
                );
            }
        }
    }
    panel
}

/// A console action button.
fn console_button(
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
            this.run(&code);
            cx.notify();
        }))
}

/// The Python console (S1: a log plus a few runnable snippets).
fn console_panel(model: &ShellModel, cx: &mut Context<Shell>) -> impl IntoElement {
    let mut log = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .px_2()
        .py_1()
        .overflow_hidden();
    for line in &model.console {
        log = log.child(div().text_xs().text_color(rgb(FG)).child(line.clone()));
    }

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
        .h(px(160.))
        .bg(rgb(PANEL))
        .child(panel_header("Python console"))
        .child(log)
        .child(actions)
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.snapshot();
        // On compositors that want client-side decorations (GNOME/Wayland), we
        // draw the title bar and the resize grips ourselves.
        let client_decorations = matches!(window.window_decorations(), Decorations::Client { .. });
        let inset = px(6.);

        let viewport = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(rgb(BG))
            .child(panel_header("3D viewport — M5 (scenix / wgpu)"))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(rgb(MUTED))
                    .child("viewport placeholder"),
            );

        let status = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .h(px(22.))
            .px_3()
            .bg(rgb(PANEL_ALT))
            .text_xs()
            .text_color(rgb(MUTED))
            .child(model.status.clone())
            .child(format!("FreeCAD {} · {}", model.version, model.backend));

        let mut inner = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(FG));
        if client_decorations {
            window.set_client_inset(inset);
            inner = inner.child(title_bar(&model, cx));
        }

        let content = inner
            .child(
                div()
                    .flex()
                    .flex_row()
                    .flex_1()
                    .min_h_0()
                    .child(tree_panel(&model, cx))
                    .child(viewport)
                    .child(properties_panel(&model)),
            )
            .child(console_panel(&model, cx))
            .child(status);

        if !client_decorations {
            return content.into_any_element();
        }

        // The backdrop turns the window border into resize grips and moves the
        // window; the content is drawn on top of it.
        div()
            .id("window-backdrop")
            .size_full()
            .child(resize_canvas(inset))
            .on_mouse_move(|_event, window, _cx| window.refresh())
            .on_mouse_down(MouseButton::Left, move |event, window, _cx| {
                let size = window.window_bounds().get_bounds().size;
                if let Some(edge) = resize_edge(event.position, inset, size) {
                    window.start_window_resize(edge);
                }
            })
            .child(content)
            .into_any_element()
    }
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

/// The custom title bar: drag to move, right-click for the window menu, and
/// platform controls on the right.
fn title_bar(model: &ShellModel, cx: &mut Context<Shell>) -> impl IntoElement {
    let controls = div()
        .flex()
        .flex_row()
        .items_center()
        .child(control_button("win-min", "\u{2013}", cx, |window| {
            window.minimize_window()
        }))
        .child(control_button("win-max", "\u{25A1}", cx, |window| {
            window.zoom_window()
        }))
        .child(control_button("win-close", "\u{2715}", cx, |window| {
            window.remove_window()
        }));

    div()
        .id("titlebar")
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .h(px(32.))
        .px_3()
        .bg(rgb(PANEL_ALT))
        .on_mouse_down(MouseButton::Left, |_event, window, _cx| {
            window.start_window_move()
        })
        .on_click(|event, window, _cx| {
            if event.is_right_click() {
                window.show_window_menu(event.position());
            }
        })
        .child(
            div()
                .text_sm()
                .text_color(rgb(FG))
                .child(model.title.clone()),
        )
        .child(controls)
}

fn control_button(
    id: &str,
    glyph: &str,
    cx: &mut Context<Shell>,
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
        .hover(|style| style.bg(rgb(0x3a3f47)))
        // Keep the title-bar drag from swallowing the click.
        .on_mouse_down(MouseButton::Left, |_event, _window, cx| {
            cx.stop_propagation()
        })
        .on_click(cx.listener(move |_this, _event, window, _cx| action(window)))
        .child(glyph.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Entity, TestAppContext, VisualTestContext, size};

    /// S1 end-to-end, headless: boot the interpreter, read the sample document
    /// through it, round-trip the console, and render the shell.
    #[gpui::test]
    fn shell_boots_lists_the_sample_document_and_is_renderable(cx: &mut TestAppContext) {
        // Tests share one CPython instance (module-global state); serialize them.
        let _guard = crate::PYTHON_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let shell = Shell::boot().expect("boot shell");
        let model = shell.model();
        {
            let m = model.lock().unwrap();
            let doc = m
                .tree
                .iter()
                .find(|d| d.name == "Shell")
                .expect("Shell document");
            let names: Vec<&str> = doc.objects.iter().map(|o| o.name.as_str()).collect();
            assert!(names.contains(&"Params"), "objects: {names:?}");
            assert!(names.contains(&"Derived"));
            assert!(m.console.iter().any(|l| l.contains("hello from Python")));
        }

        // The property editor is fed from the live model.
        let props = crate::python::properties("Params").expect("properties");
        assert!(props.iter().any(|p| p.name == "Length"));

        // The console round-trips a snippet through the interpreter.
        let out = crate::python::evaluate("len(doc.Objects)").expect("evaluate");
        assert!(out.ok, "eval failed: {}", out.output);
        assert_eq!(out.output, "3");

        // The shell renders headlessly (no display server, no GPU).
        let window = cx.update(|cx| {
            let model = model.clone();
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|_| Shell::from_model(model.clone()))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let _root: Entity<Shell> = window.root(&mut cx).unwrap();
    }

    /// Pure geometry: the resize-grip hit regions hug the border and leave the
    /// content area alone.
    #[test]
    fn resize_edge_detects_corners_edges_and_content() {
        let inset = px(6.);
        let size = size(px(1000.), px(800.));

        // Corners take precedence over the plain edges they touch.
        assert_eq!(resize_edge(point(px(2.), px(2.)), inset, size), Some(ResizeEdge::TopLeft));
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
        assert_eq!(resize_edge(point(px(500.), px(2.)), inset, size), Some(ResizeEdge::Top));
        assert_eq!(
            resize_edge(point(px(500.), px(798.)), inset, size),
            Some(ResizeEdge::Bottom)
        );
        assert_eq!(resize_edge(point(px(2.), px(400.)), inset, size), Some(ResizeEdge::Left));
        assert_eq!(
            resize_edge(point(px(998.), px(400.)), inset, size),
            Some(ResizeEdge::Right)
        );

        // The grip band is the outer `inset` pixels: x < inset is a grip, and
        // the first pixel at or past `inset` is content.
        assert_eq!(resize_edge(point(px(5.), px(400.)), inset, size), Some(ResizeEdge::Left));
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
        assert_eq!(cursor_for(ResizeEdge::TopLeft), CursorStyle::ResizeUpLeftDownRight);
        assert_eq!(
            cursor_for(ResizeEdge::BottomRight),
            CursorStyle::ResizeUpLeftDownRight
        );
        assert_eq!(cursor_for(ResizeEdge::TopRight), CursorStyle::ResizeUpRightDownLeft);
        assert_eq!(
            cursor_for(ResizeEdge::BottomLeft),
            CursorStyle::ResizeUpRightDownLeft
        );
    }
}
