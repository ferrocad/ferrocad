//! Feasibility spike: a Python workbench *declares* a UI tree; Rust parses it
//! and renders it with `bite-gpui`, wiring real `on_click` handlers back to
//! Python.
//!
//! Covers the four criteria from `docs/python-ui-research.md` Part D:
//!   1. the Rust host embeds CPython,
//!   2. the Python-declared tree compiles into `bite-gpui` elements headlessly,
//!   3. a real click round-trips through a Python callback that mutates state,
//!   4. a raising Python handler is isolated at the boundary without crashing the host.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui::{div, prelude::*, px, AnyElement, Context, IntoElement, Render, SharedString, Window};
use pyo3::types::PyAnyMethods;
use serde::Deserialize;

/// One node of the Python-declared UI, as it crosses the PyO3 boundary.
#[derive(Debug, Clone, Deserialize)]
pub struct WireNode {
    pub id: String,
    pub tag: String,
    #[serde(default)]
    pub props: HashMap<String, String>,
    #[serde(default)]
    pub events: Vec<(String, String)>,
    #[serde(default)]
    pub children: Vec<WireNode>,
}

/// One diffed patch from the Python side (a minimal change stream).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum Patch {
    Insert {
        parent: String,
        index: usize,
        node: WireNode,
    },
    Remove { id: String },
    Update { id: String, props: HashMap<String, String> },
}

fn find_mut<'a>(node: &'a mut WireNode, id: &str) -> Option<&'a mut WireNode> {
    if node.id == id {
        return Some(node);
    }
    for child in &mut node.children {
        if let Some(found) = find_mut(child, id) {
            return Some(found);
        }
    }
    None
}

fn remove_by_id(node: &mut WireNode, id: &str) -> bool {
    let before = node.children.len();
    node.children.retain(|c| c.id != id);
    if node.children.len() != before {
        return true;
    }
    for child in &mut node.children {
        if remove_by_id(child, id) {
            return true;
        }
    }
    false
}

fn insert_at(root: &mut WireNode, parent: &str, index: usize, node: &WireNode) -> bool {
    if root.id == parent {
        let idx = index.min(root.children.len());
        root.children.insert(idx, node.clone());
        return true;
    }
    for child in &mut root.children {
        if insert_at(child, parent, index, node) {
            return true;
        }
    }
    false
}

fn apply_patches(root: &mut WireNode, patches: &[Patch]) {
    for patch in patches {
        match patch {
            Patch::Update { id, props } => {
                if let Some(node) = find_mut(root, id) {
                    for (k, v) in props {
                        node.props.insert(k.clone(), v.clone());
                    }
                }
            }
            Patch::Remove { id } => {
                remove_by_id(root, id);
            }
            Patch::Insert { parent, index, node } => {
                insert_at(root, parent, *index, node);
            }
        }
    }
}

fn python_path() -> String {
    std::env::var("FC_SPIKE_PYTHON_PATH").unwrap_or_else(|_| "../../python".to_string())
}

/// Criterion 1: embed CPython, put `python/` on the path, render the tree.
fn initialize_python_runtime() -> Result<WireNode, String> {
    let json: String = pyo3::Python::with_gil(|py| -> Result<String, String> {
        let sys = py.import("sys").map_err(|e| e.to_string())?;
        let path = sys.getattr("path").map_err(|e| e.to_string())?;
        path.call_method1("insert", (0, python_path()))
            .map_err(|e| e.to_string())?;

        let module = py
            .import("ferrocad_spike.declarative")
            .map_err(|e| e.to_string())?;
        module
            .getattr("reset_state")
            .map_err(|e| e.to_string())?
            .call0()
            .map_err(|e| e.to_string())?;
        let raw: String = module
            .getattr("render_ui")
            .map_err(|e| e.to_string())?
            .call0()
            .map_err(|e| e.to_string())?
            .extract()
            .map_err(|e| e.to_string())?;
        Ok(raw)
    })?;

    serde_json::from_str(&json).map_err(|e| e.to_string())
}

/// Call `ferrocad_spike.declarative.dispatch_event(handler_id)`; return fresh JSON,
/// or an `exception: …` error marker when the Python handler raises.
fn call_dispatch_event(handler_id: &str) -> Result<String, String> {
    pyo3::Python::with_gil(|py| {
        let module = py
            .import("ferrocad_spike.declarative")
            .map_err(|e| e.to_string())?;
        let func = module
            .getattr("dispatch_event")
            .map_err(|e| e.to_string())?;
        match func.call1((handler_id,)) {
            Ok(value) => value.extract::<String>().map_err(|e| e.to_string()),
            Err(e) => Err(format!("exception: {e}")),
        }
    })
}

/// A view that renders the Python-declared tree and dispatches clicks back to
/// Python. The tree is shared via an `Arc<Mutex<_>>` so a click handler can
/// replace it and notify for a re-render.
pub struct WireView {
    tree: Arc<Mutex<Option<WireNode>>>,
}

impl WireView {
    pub fn new(tree: Arc<Mutex<Option<WireNode>>>) -> Self {
        Self { tree }
    }

    /// Criteria 3 & 4: run a Python handler, apply its diffed patches; isolate
    /// a raising one. Returns the number of patches applied.
    fn dispatch(&mut self, handler_id: &str) -> Result<usize, String> {
        match call_dispatch_event(handler_id) {
            Ok(json) => {
                let patches: Vec<Patch> =
                    serde_json::from_str(&json).map_err(|e| e.to_string())?;
                let count = patches.len();
                let mut guard = self.tree.lock().unwrap();
                if let Some(root) = guard.as_mut() {
                    apply_patches(root, &patches);
                }
                Ok(count)
            }
            Err(msg) if msg.starts_with("exception:") => {
                eprintln!("[FAULT ISOLATED] {msg}");
                // Keep the previous tree; the host stays stable.
                Ok(0)
            }
            Err(msg) => Err(msg),
        }
    }

    fn element(&mut self, node: &WireNode, cx: &mut Context<Self>) -> AnyElement {
        let mut el = div().id(SharedString::from(node.id.clone()));
        match node.tag.as_str() {
            "label" => {
                if let Some(text) = node.props.get("text") {
                    el = el.h(px(20.)).child(text.clone());
                }
            }
            "button" => {
                el = el.h(px(30.)).w(px(160.));
                if let Some(title) = node.props.get("title") {
                    el = el.child(title.clone());
                }
                if let Some(handler_id) = node
                    .events
                    .iter()
                    .find(|(evt, _)| evt == "click")
                    .map(|(_, id)| id.clone())
                {
                    el = el.on_click(cx.listener(move |this, _ev, _window, cx| {
                        let _ = this.dispatch(&handler_id);
                        cx.notify();
                    }));
                }
            }
            _ => {}
        }
        for child in &node.children {
            let child_el = self.element(child, cx);
            el = el.child(child_el);
        }
        el.into_any_element()
    }
}

impl Render for WireView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let node = self.tree.lock().unwrap().clone();
        match node {
            Some(n) => self.element(&n, cx),
            None => div().into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Bounds, Entity, Modifiers, Point, TestAppContext, VisualTestContext, WindowBounds,
        WindowOptions, px, size,
    };

    fn tree_with_python() -> Arc<Mutex<Option<WireNode>>> {
        let node = initialize_python_runtime().expect("embed python");
        Arc::new(Mutex::new(Some(node)))
    }

    fn text(tree: &Arc<Mutex<Option<WireNode>>>) -> String {
        tree.lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .children[0]
            .props["text"]
            .clone()
    }

    fn child_count(tree: &Arc<Mutex<Option<WireNode>>>) -> usize {
        tree.lock().unwrap().as_ref().unwrap().children.len()
    }

    /// Pure-Rust: the patch stream deserializes and applies (no Python, no UI).
    #[test]
    fn patch_application_updates_inserts_and_removes() {
        let mut root = WireNode {
            id: "root".into(),
            tag: "container".into(),
            props: std::collections::HashMap::new(),
            events: vec![],
            children: vec![
                WireNode {
                    id: "counter_label".into(),
                    tag: "label".into(),
                    props: std::collections::HashMap::from([(
                        "text".into(),
                        "Total Operations: 0".into(),
                    )]),
                    events: vec![],
                    children: vec![],
                },
                WireNode {
                    id: "btn_inc".into(),
                    tag: "button".into(),
                    props: std::collections::HashMap::from([(
                        "title".into(),
                        "Increment Count".into(),
                    )]),
                    events: vec![],
                    children: vec![],
                },
                WireNode {
                    id: "btn_rem".into(),
                    tag: "button".into(),
                    props: std::collections::HashMap::from([(
                        "title".into(),
                        "Remove Item".into(),
                    )]),
                    events: vec![],
                    children: vec![],
                },
            ],
        };

        let patches: Vec<Patch> = serde_json::from_str(
            r#"[
                {"op":"update","id":"counter_label","props":{"text":"Total Operations: 1"}},
                {"op":"remove","id":"btn_inc"},
                {"op":"insert","parent":"root","index":1,"node":{"id":"item_0","tag":"label","props":{"text":"Item 1"}}}
            ]"#,
        )
        .unwrap();

        apply_patches(&mut root, &patches);

        assert_eq!(root.children[0].props["text"], "Total Operations: 1");
        assert_eq!(root.children[1].id, "item_0"); // inserted at index 1
        assert_eq!(root.children[2].id, "btn_rem"); // btn_inc removed
        assert_eq!(root.children.len(), 3);
    }

    /// Criteria 1–4 in one headless test.
    ///
    /// A single test drives the embedded interpreter because tests that share
    /// one CPython instance (module-global state) must be serialized — see
    /// `docs/python-ui-research.md` §F.
    #[gpui::test]
    fn python_declares_ui_and_click_round_trips(cx: &mut TestAppContext) {
        // Tests share one CPython instance (module-global state); serialize them.
        let _guard = crate::PYTHON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let tree = tree_with_python();
        assert_eq!(text(&tree), "Total Operations: 0");
        assert_eq!(child_count(&tree), 5); // label + 4 buttons

        let window = cx.update(|cx| {
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::new(px(0.), px(0.)),
                        size: size(px(400.), px(300.)),
                    })),
                    ..Default::default()
                },
                |_, cx| cx.new(|_| WireView::new(tree.clone())),
            )
            .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);
        let _root: Entity<WireView> = window.root(&mut cx).unwrap();

        // Stacked block layout: label y=0..20, btn_inc 20..50, btn_add 50..80,
        // btn_rem 80..110, btn_err 110..140 (x=0..160).
        cx.simulate_click(Point::new(px(80.), px(35.)), Modifiers::none());
        assert_eq!(text(&tree), "Total Operations: 1");

        cx.simulate_click(Point::new(px(80.), px(65.)), Modifiers::none());
        assert_eq!(child_count(&tree), 6); // item_0 inserted

        cx.simulate_click(Point::new(px(80.), px(95.)), Modifiers::none());
        assert_eq!(child_count(&tree), 5); // item_0 removed

        // btn_err raises in Python; isolated, state unchanged.
        cx.simulate_click(Point::new(px(80.), px(125.)), Modifiers::none());
        assert_eq!(text(&tree), "Total Operations: 1");
        assert_eq!(child_count(&tree), 5);
    }
}
