//! The app-shell view (S1).
//!
//! One window with a title bar, a model inspector, a viewport placeholder, a
//! property editor and a Python console. The layout and the interpreter wiring
//! are the S1 deliverable: the tree, editor and console are already driven by
//! the live document through `python/ferrocad_shell`; richer widgets, commands
//! and menus come in later slices (see `docs/app-shell-vision.md`).

use std::sync::{Arc, Mutex};

use gpui::{
    App, Context, Entity, FocusHandle, Focusable, IntoElement, Render, ScrollHandle, SharedString,
    Subscription, Window, div, prelude::*, px, rgb,
};

use crate::input::{SubmitEvent, TextInput};
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
    /// The editable Python console line (S5's input, previewed here).
    console_input: Entity<TextInput>,
    /// Keeps the console log pinned to the latest line.
    console_scroll: ScrollHandle,
    _submit: Subscription,
}

impl Shell {
    /// Boot the interpreter, create the sample document, and return the model the
    /// view is built from. The view itself is created with `from_model` because
    /// entities (the console input) need an `App` context.
    pub fn boot() -> Result<Arc<Mutex<ShellModel>>, String> {
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
        Ok(Arc::new(Mutex::new(ShellModel {
            title: format!("FerroCAD — {}", boot.document),
            version: boot.version,
            backend: boot.backend,
            tree,
            selected: None,
            properties: Vec::new(),
            console,
            status,
        })))
    }

    /// Build a shell around an existing model (the window and tests use this).
    pub fn from_model(model: Arc<Mutex<ShellModel>>, cx: &mut Context<Self>) -> Self {
        let console_input =
            cx.new(|cx| TextInput::new(cx, ">>> type Python and press Enter"));
        let submit = cx.subscribe(
            &console_input,
            |this: &mut Shell, emitter, event: &SubmitEvent, cx| {
                this.run(&event.text);
                emitter.update(cx, |input, cx| input.clear(cx));
                cx.notify();
            },
        );
        Self {
            model,
            console_input,
            console_scroll: ScrollHandle::new(),
            _submit: submit,
        }
    }

    /// The console's focus handle, so the window can put the caret there at boot.
    pub fn console_focus_handle(&self, cx: &App) -> FocusHandle {
        self.console_input.read(cx).focus_handle(cx)
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
        drop(m);
        // New output arrives at the bottom; keep it in view.
        self.console_scroll.scroll_to_bottom();
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
    let mut rows = div()
        .id("tree-scroll")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll();

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
            if model.selected.as_deref() == Some(name.as_str()) {
                row = row.bg(rgb(ACCENT));
            }
            rows = rows.child(row.on_click(cx.listener(move |this, _ev, _win, cx| {
                this.select(&name);
                cx.notify();
            })));
        }
    }

    div()
        .flex()
        .flex_col()
        .w(px(260.))
        .h_full()
        .bg(rgb(PANEL))
        .child(panel_header("Model"))
        .child(rows)
}

/// The property editor (S1: read-only rows for the selected object).
fn properties_panel(model: &ShellModel) -> impl IntoElement {
    let mut rows = div()
        .id("properties-scroll")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll();

    match &model.selected {
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
                    .child(name.clone()),
            );
            for row in &model.properties {
                let value = if row.status.is_empty() {
                    row.value.clone()
                } else {
                    format!("{}  [{}]", row.value, row.status)
                };
                rows = rows.child(
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

    div()
        .flex()
        .flex_col()
        .w(px(320.))
        .h_full()
        .bg(rgb(PANEL))
        .child(panel_header("Properties"))
        .child(rows)
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

/// The Python console: a log, an editable input line, and a few runnable snippets.
fn console_panel(
    model: &ShellModel,
    input: &Entity<TextInput>,
    scroll: &ScrollHandle,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
    let mut log = div()
        .id("console-scroll")
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .px_2()
        .py_1()
        .overflow_y_scroll()
        .track_scroll(scroll);
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
        .child(input.clone())
        .child(actions)
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.snapshot();

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

        let content = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(rgb(BG))
            .text_color(rgb(FG))
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
            .child(console_panel(
                &model,
                &self.console_input,
                &self.console_scroll,
                cx,
            ))
            .child(status);

        crate::chrome::window_frame(window, model.title.clone(), content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Entity, TestAppContext, VisualTestContext};

    /// S1 end-to-end, headless: boot the interpreter, read the sample document
    /// through it, round-trip the console, and render the shell.
    #[gpui::test]
    fn shell_boots_lists_the_sample_document_and_is_renderable(cx: &mut TestAppContext) {
        // Tests share one CPython instance (module-global state); serialize them.
        let _guard = crate::PYTHON_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());

        let model = Shell::boot().expect("boot shell");
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
                cx.new(|cx| Shell::from_model(model.clone(), cx))
            })
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();

        // Typing in the console input and pressing Enter runs the line through the
        // interpreter (S5 previewed): the log gains the echoed line and the result,
        // and the field is cleared.
        let console = shell.read_with(&cx, |shell, cx| shell.console_focus_handle(cx));
        cx.update(|window, cx| window.focus(&console, cx));
        cx.simulate_input("len(doc.Objects)");
        cx.simulate_keystrokes("enter");
        {
            let m = model.lock().unwrap();
            assert!(
                m.console.iter().any(|l| l == ">>> len(doc.Objects)"),
                "console: {:?}",
                m.console
            );
            assert!(m.console.iter().any(|l| l == "3"), "console: {:?}", m.console);
        }
    }
}
