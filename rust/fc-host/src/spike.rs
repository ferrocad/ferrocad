//! Feasibility spike: a Python workbench *declares* a UI tree; Rust parses it
//! and renders it with `bite-gpui`.
//!
//! Covers the four criteria from `docs/python-ui-research.md` Part D:
//!   1. the Rust host embeds CPython,
//!   2. the Python-declared tree compiles into `bite-gpui` elements headlessly,
//!   3. a synthetic event round-trips through a Python callback that mutates state,
//!   4. a raising Python handler is isolated at the boundary without crashing the host.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use gpui::{div, prelude::*, AnyElement, Context, IntoElement, Render, SharedString, Window};
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
    #[allow(dead_code)] // parsed now, wired to on_click handlers in the next step
    pub events: Vec<(String, String)>,
    #[serde(default)]
    pub children: Vec<WireNode>,
}

/// Convert a `WireNode` into a `bite-gpui` element tree. Only the handful of
/// tags the spike uses are mapped; this is where the real "UI abstraction"
/// would live.
fn wire_to_element(node: &WireNode) -> AnyElement {
    let mut el = div().id(SharedString::from(node.id.clone()));
    match node.tag.as_str() {
        "label" => {
            if let Some(text) = node.props.get("text") {
                el = el.child(text.clone());
            }
        }
        "button" => {
            if let Some(title) = node.props.get("title") {
                el = el.child(title.clone());
            }
        }
        _ => {}
    }
    for child in &node.children {
        el = el.child(wire_to_element(child));
    }
    el.into_any_element()
}

/// Owns the embedded interpreter state: the current tree, shared with the view.
pub struct Host {
    pub tree: Arc<Mutex<Option<WireNode>>>,
}

impl Host {
    pub fn new() -> Self {
        Self {
            tree: Arc::new(Mutex::new(None)),
        }
    }

    fn python_path() -> String {
        std::env::var("FC_SPIKE_PYTHON_PATH").unwrap_or_else(|_| "../../python".to_string())
    }

    /// Criterion 1: embed CPython, put `python/` on the path, render the tree.
    pub fn initialize_python_runtime(&self) -> Result<(), String> {
        let json: String = pyo3::Python::with_gil(|py| -> Result<String, String> {
            let sys = py.import("sys").map_err(|e| e.to_string())?;
            let path = sys.getattr("path").map_err(|e| e.to_string())?;
            path.call_method1("insert", (0, Self::python_path()))
                .map_err(|e| e.to_string())?;

            let module = py
                .import("fcspike.declarative")
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

        let node: WireNode = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        *self.tree.lock().unwrap() = Some(node);
        Ok(())
    }

    /// Criteria 3 & 4: dispatch an event into Python; isolate a raising handler.
    pub fn trigger_event(&self, handler_id: &str) -> Result<(), String> {
        let result: Result<String, String> = pyo3::Python::with_gil(|py| {
            let module = py
                .import("fcspike.declarative")
                .map_err(|e| e.to_string())?;
            let func = module
                .getattr("dispatch_event")
                .map_err(|e| e.to_string())?;
            match func.call1((handler_id,)) {
                Ok(value) => value.extract::<String>().map_err(|e| e.to_string()),
                Err(e) => Err(format!("exception: {e}")),
            }
        });

        match result {
            Ok(json) => {
                let node: WireNode = serde_json::from_str(&json).map_err(|e| e.to_string())?;
                *self.tree.lock().unwrap() = Some(node);
                Ok(())
            }
            Err(msg) if msg.starts_with("exception:") => {
                eprintln!("[FAULT ISOLATED] {msg}");
                // Keep the previous tree; the host stays stable.
                Ok(())
            }
            Err(msg) => Err(msg),
        }
    }

    pub fn current(&self) -> Option<WireNode> {
        self.tree.lock().unwrap().clone()
    }
}

/// A view that renders the current Python-declared tree.
pub struct WireView {
    tree: Arc<Mutex<Option<WireNode>>>,
}

impl Render for WireView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let node = self.tree.lock().unwrap().clone();
        match node {
            Some(n) => wire_to_element(&n),
            None => div().into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Entity, TestAppContext, VisualTestContext};

    /// Criteria 1, 3, 4 — plain Python round-trip + exception isolation.
    #[test]
    fn python_roundtrip_and_exception_isolation() {
        let host = Host::new();
        host.initialize_python_runtime().expect("embed python");

        let tree = host.current().expect("initial tree");
        assert_eq!(tree.tag, "container");
        assert_eq!(
            tree.children[0].props.get("text").unwrap(),
            "Total Operations: 0"
        );

        host.trigger_event("btn_inc_click").expect("increment");
        assert_eq!(
            host.current().unwrap().children[0].props["text"],
            "Total Operations: 1"
        );

        // A raising handler is isolated; state is unchanged, host is stable.
        host.trigger_event("btn_err_click").expect("fault isolated");
        assert_eq!(
            host.current().unwrap().children[0].props["text"],
            "Total Operations: 1"
        );
    }

    /// Criterion 2 — the Python-declared tree renders in a headless window.
    #[gpui::test]
    fn python_declares_ui_renders_headless(cx: &mut TestAppContext) {
        let host = Host::new();
        host.initialize_python_runtime().expect("embed python");

        let tree = host.tree.clone();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |_, cx| cx.new(|_| WireView { tree }))
                .unwrap()
        });
        let mut cx = VisualTestContext::from_window(window.into(), cx);

        // No display server, no GPU: the view mounts and renders.
        let _root: Entity<WireView> = window.root(&mut cx).unwrap();
    }
}
