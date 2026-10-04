//! The app-shell view (S1).
//!
//! One window with a title bar, a model inspector, a viewport placeholder, a
//! property editor and a Python console. The layout and the interpreter wiring
//! are the S1 deliverable: the tree, editor and console are already driven by
//! the live document through `python/ferrocad_shell`; richer widgets, commands
//! and menus come in later slices (see `docs/app-shell-vision.md`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui::{
    AnyElement, App, Context, Entity, FocusHandle, Focusable, IntoElement, Render, SharedString,
    Subscription, Window, div, prelude::*, px, rgb,
};

use ferrocad_ui::{SubmitEvent, TextAreaState, TextInput, window_frame};

use crate::python::{self, Bootstrap, DocumentNode, PropRow};

const BG: u32 = 0x101216;
const PANEL: u32 = 0x16181d;
const PANEL_ALT: u32 = 0x1d2026;
const FG: u32 = 0xd8dbe0;
const MUTED: u32 = 0x8b919a;
const ACCENT: u32 = 0x2f4a6d;

/// The console transcript: the log lines plus a fresh prompt.
fn transcript_of(model: &Arc<Mutex<ShellModel>>) -> String {
    let m = model.lock().unwrap();
    format!("{}\n>>> ", m.console.join("\n"))
}

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
    /// The console transcript plus the live input line (S5's text area).
    console: Entity<TextAreaState>,
    /// One uncontrolled field per editable property, keyed by `object.property`.
    prop_inputs: HashMap<String, PropInput>,
    _submit: Subscription,
}

/// An uncontrolled property field plus the value it was last seeded with.
struct PropInput {
    state: Entity<TextInput>,
    seeded: String,
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
        let transcript = transcript_of(&model);
        let console = cx.new(|cx| {
            let mut area = TextAreaState::new(cx);
            area.set_transcript(transcript, cx);
            area
        });
        let submit = cx.subscribe(
            &console,
            |this: &mut Shell, _emitter, event: &SubmitEvent, cx| {
                this.run(&event.text, cx);
            },
        );
        Self {
            model,
            console,
            prop_inputs: HashMap::new(),
            _submit: submit,
        }
    }

    /// The console's focus handle, so the window can put the caret there at boot.
    pub fn console_focus_handle(&self, cx: &App) -> FocusHandle {
        self.console.read(cx).focus_handle(cx)
    }

    /// Select an object; the property editor is filled from the live model. The
    /// property fields are dropped so they re-seed from the new object's values.
    fn select(&mut self, name: &str) {
        let properties = python::properties(name).unwrap_or_default();
        self.prop_inputs.clear();
        let mut m = self.model.lock().unwrap();
        m.selected = Some(name.to_string());
        m.properties = properties;
        m.status = format!("selected {name}");
    }

    /// Re-read the tree and the selected object's properties after a change.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        let selected = self.model.lock().unwrap().selected.clone();
        let tree = python::model_tree().unwrap_or_default();
        let properties = selected
            .as_deref()
            .map(|name| python::properties(name).unwrap_or_default())
            .unwrap_or_default();
        {
            let mut m = self.model.lock().unwrap();
            m.tree = tree;
            m.properties = properties;
        }
        cx.notify();
    }

    /// Get the field for a property, creating it (and its submit subscription) on
    /// first use. The field is re-seeded from the model when the value changed
    /// underneath and the user is not editing it.
    fn ensure_prop_input(
        &mut self,
        key: &str,
        value: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        if let Some(input) = self.prop_inputs.get_mut(key) {
            let focused = input.state.read(cx).focus_handle(cx).is_focused(window);
            if input.seeded != value && !focused {
                input
                    .state
                    .update(cx, |state, cx| state.set_text(value.to_string(), cx));
                input.seeded = value.to_string();
            }
            return input.state.clone();
        }

        let state = cx.new(|cx| {
            let mut state = TextInput::new(cx, "");
            state.set_text(value.to_string(), cx);
            state
        });
        let key = key.to_string();
        let key_for_submit = key.clone();
        let submit = cx.subscribe(
            &state,
            move |this: &mut Shell, _emitter, event: &SubmitEvent, cx| {
                this.commit_property(&key_for_submit, &event.text, cx);
            },
        );
        self.prop_inputs.insert(
            key,
            PropInput {
                state: state.clone(),
                seeded: value.to_string(),
                _submit: submit,
            },
        );
        state
    }

    /// Commit a property field: one filtered edit inside one transaction.
    fn commit_property(&mut self, key: &str, text: &str, cx: &mut Context<Self>) {
        let Some((object, property)) = key.split_once('.') else {
            return;
        };
        let status = match python::set_property(object, property, text) {
            Ok(result) if result.ok => {
                if let Some(input) = self.prop_inputs.get_mut(key) {
                    input.seeded = text.to_string();
                }
                self.refresh(cx);
                format!("set {key}")
            }
            Ok(result) => format!("property error: {}", result.error),
            Err(e) => format!("property error: {e}"),
        };
        self.model.lock().unwrap().status = status;
        cx.notify();
    }

    /// Run a console snippet, refresh the tree (the model may have changed), and
    /// rebuild the transcript so the console text area shows the result and a
    /// fresh prompt.
    fn run(&mut self, code: &str, cx: &mut Context<Self>) {
        let result = python::evaluate(code);
        {
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
        }
        // A console line can add objects or change values; re-read the model.
        self.refresh(cx);
        let transcript = transcript_of(&self.model);
        self.console
            .update(cx, |area, cx| area.set_transcript(transcript, cx));
        cx.notify();
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

/// The property editor: read-only rows plus an uncontrolled field for each
/// editable property.
fn properties_panel(
    shell: &mut Shell,
    model: &ShellModel,
    window: &Window,
    cx: &mut Context<Shell>,
) -> impl IntoElement {
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
                let value: AnyElement = if row.editable {
                    let key = format!("{name}.{}", row.name);
                    let state = shell.ensure_prop_input(&key, &row.value, window, cx);
                    div()
                        .w(px(150.))
                        .flex_shrink_0()
                        .child(state)
                        .into_any_element()
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
            this.run(&code, cx);
        }))
}

/// The Python console: one text area (scrollback plus prompt) and runnable
/// snippets.
fn console_panel(console: &Entity<TextAreaState>, cx: &mut Context<Shell>) -> impl IntoElement {
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
        .child(console.clone())
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
                    .child(properties_panel(self, &model, window, cx)),
            )
            .child(console_panel(&self.console, cx))
            .child(status);

        window_frame(window, model.title.clone(), content)
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

    /// Slice E: an uncontrolled property field commits on Enter, inside a named
    /// transaction, and the change propagates through recompute.
    #[gpui::test]
    fn editing_a_property_commits_in_a_transaction(cx: &mut TestAppContext) {
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
        let shell: Entity<Shell> = window.root(&mut cx).unwrap();

        // Select Params so the editor builds one field per editable property.
        cx.update(|_window, cx| {
            shell.update(cx, |shell, cx| {
                shell.select("Params");
                cx.notify();
            });
        });

        let field = shell.read_with(&cx, |shell, _| {
            shell
                .prop_inputs
                .get("Params.Length")
                .expect("Length field")
                .state
                .clone()
        });
        cx.update(|window, cx| window.focus(&field.read(cx).focus_handle(cx), cx));
        cx.simulate_keystrokes("cmd-a");
        cx.simulate_input("25 mm");
        cx.simulate_keystrokes("enter");

        // The selected object's row reflects the committed value...
        let length = shell.read_with(&cx, |shell, _| {
            shell
                .model
                .lock()
                .unwrap()
                .properties
                .iter()
                .find(|row| row.name == "Length")
                .map(|row| row.value.clone())
        });
        assert!(
            length.as_deref().unwrap_or_default().contains("25"),
            "Length = {length:?}"
        );

        // ...and recompute propagated it to the expression-driven other object.
        let derived = crate::python::properties("Derived").expect("properties");
        let result = derived
            .iter()
            .find(|row| row.name == "Result")
            .map(|row| row.value.clone())
            .unwrap_or_default();
        assert!(result.contains("50"), "Derived.Result = {result:?}");
    }
}
