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
            .import("fcspike.declarative")
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

/// Call `fcspike.declarative.dispatch_event(handler_id)`; return fresh JSON,
/// or an `exception: …` error marker when the Python handler raises.
fn call_dispatch_event(handler_id: &str) -> Result<String, String> {
    pyo3::Python::with_gil(|py| {
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

    /// Criteria 3 & 4: run a Python handler; isolate a raising one.
    fn dispatch(&mut self, handler_id: &str) {
        match call_dispatch_event(handler_id) {
            Ok(json) => {
                if let Ok(node) = serde_json::from_str::<WireNode>(&json) {
                    *self.tree.lock().unwrap() = Some(node);
                }
            }
            Err(msg) if msg.starts_with("exception:") => {
                eprintln!("[FAULT ISOLATED] {msg}");
                // Keep the previous tree; the host stays stable.
            }
            Err(msg) => eprintln!("[DISPATCH ERROR] {msg}"),
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
                        this.dispatch(&handler_id);
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

    /// Criteria 1–4 in one headless test.
    ///
    /// A single test drives the embedded interpreter because tests that share
    /// one CPython instance (module-global state) must be serialized — see
    /// `docs/python-ui-research.md` §F.
    #[gpui::test]
    fn python_declares_ui_and_click_round_trips(cx: &mut TestAppContext) {
        let tree = tree_with_python();
        assert_eq!(text(&tree), "Total Operations: 0");

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

        // Stacked block layout: label y=0..20, btn_inc y=20..50, btn_err y=50..80.
        cx.simulate_click(Point::new(px(80.), px(35.)), Modifiers::none());
        assert_eq!(text(&tree), "Total Operations: 1");

        // btn_err raises in Python; isolated, state unchanged.
        cx.simulate_click(Point::new(px(80.), px(65.)), Modifiers::none());
        assert_eq!(text(&tree), "Total Operations: 1");
    }
}
