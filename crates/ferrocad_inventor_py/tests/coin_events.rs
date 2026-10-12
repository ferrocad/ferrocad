//! S4 tests: the event classes, `SoEventCallback`, and the dispatch bridge.
//!
//! A workbench registers a callback on a `SoEventCallback` node; the host dispatches
//! each input event (with its pick) through it, and the callback reads the event and
//! the point it hit via `getEvent()`/`getPickedPoint()`.

use ferrocad_inventor::{Event, Hit, PickId};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict};

/// Build a scene whose only child is an `SoEventCallback` that records what it saw.
const SCRIPT: &str = r#"
from pivy import coin

seen = []
errors = []

def on_button(event_cb):
    event = event_cb.getEvent()
    pp = event_cb.getPickedPoint()
    if pp is None:
        seen.append(("no-pick", type(event).__name__))
    else:
        x, y, z = pp.getPoint()
        nx, ny, nz = pp.getNormal()
        detail = pp.getDetail()
        seen.append((
            type(event).__name__, event.getButton(), event.getState(),
            x, y, z, detail.getFaceIndex(), detail.getPartIndex(),
            isinstance(detail, coin.SoFaceDetail),
        ))

def on_move(event_cb):
    seen.append(("move", type(event_cb.getEvent()).__name__))

root = coin.SoSeparator()
cb = coin.SoEventCallback()
cb.addEventCallback(coin.SoMouseButtonEvent.getClassTypeId(), on_button)
cb.addEventCallback(coin.SoLocation2Event.getClassTypeId(), on_move)
root.addChild(cb)
"#;

/// Run `SCRIPT` and return `(root, globals)`.
fn build(py: Python<'_>) -> (Bound<'_, PyAny>, Bound<'_, PyDict>) {
    ferrocad_inventor_py::install(py).expect("install coin");
    let globals = PyDict::new(py);
    let code = std::ffi::CString::new(SCRIPT).expect("script has no NUL");
    py.run(&code, Some(&globals), None).expect("run scene script");
    let root = globals
        .get_item("root")
        .expect("get_item")
        .expect("script defines root");
    (root, globals)
}

fn hit() -> Hit {
    Hit {
        id: PickId(0),
        point: [1.0, 2.0, 3.0],
        normal: [0.0, 1.0, 0.0],
        triangle: 4,
        distance: 5.0,
    }
}

#[test]
fn dispatch_delivers_the_event_and_pick_to_the_matching_callback() {
    Python::with_gil(|py| {
        let (root, globals) = build(py);
        let scene = ferrocad_inventor_py::extract_scene(&root).expect("extract");

        let event = Event::MouseButton {
            position: [10, 20],
            button: 1,
            down: true,
        };
        let fired = ferrocad_inventor_py::dispatch_event(py, &scene, &event, Some(&hit()))
            .expect("dispatch");
        assert!(fired, "a button callback should have run");

        // Only the button callback matched (not the location one).
        let seen: Vec<(String, i32, i32, f32, f32, f32, i32, i32, bool)> = globals
            .get_item("seen")
            .unwrap()
            .unwrap()
            .extract()
            .unwrap();
        assert_eq!(seen.len(), 1, "exactly one callback: {seen:?}");
        let (class, button, state, x, y, z, face, part, is_face) = &seen[0];
        assert_eq!(class, "SoMouseButtonEvent");
        assert_eq!(*button, 1);
        assert_eq!(*state, 1, "DOWN");
        assert_eq!((*x, *y, *z), (1.0, 2.0, 3.0));
        assert_eq!(*face, 4);
        assert_eq!(*part, 0);
        assert!(*is_face, "detail should be an SoFaceDetail");
    });
}

#[test]
fn dispatch_clears_the_context_after_the_call() {
    Python::with_gil(|py| {
        let (root, globals) = build(py);
        let scene = ferrocad_inventor_py::extract_scene(&root).expect("extract");
        let event = Event::MouseButton {
            position: [1, 1],
            button: 3,
            down: false,
        };
        ferrocad_inventor_py::dispatch_event(py, &scene, &event, None).expect("dispatch");

        // The callback saw no pick (the dispatch carried none)...
        let seen: Vec<(String, String)> = globals
            .get_item("seen")
            .unwrap()
            .unwrap()
            .extract()
            .unwrap();
        assert_eq!(seen, vec![("no-pick".to_string(), "SoMouseButtonEvent".to_string())]);

        // ...and the node's context is clear once the dispatch returns.
        let cb = globals.get_item("cb").unwrap().unwrap();
        assert!(cb.call_method0("getEvent").unwrap().is_none());
        assert!(cb.call_method0("getPickedPoint").unwrap().is_none());
    });
}

#[test]
fn event_classes_and_constants() {
    Python::with_gil(|py| {
        let coin = ferrocad_inventor_py::install(py).expect("install coin");
        let mbb = coin.getattr("SoMouseButtonEvent").unwrap();
        // Coin-style constants and the type hierarchy.
        assert_eq!(mbb.getattr("BUTTON1").unwrap().extract::<i32>().unwrap(), 1);
        assert_eq!(
            coin.getattr("SoButtonEvent").unwrap().getattr("DOWN").unwrap().extract::<i32>().unwrap(),
            1
        );
        assert!(mbb
            .call_method0("getClassTypeId")
            .unwrap()
            .call_method0("getName")
            .unwrap()
            .extract::<String>()
            .unwrap()
            == "SoMouseButtonEvent");
        assert!(mbb
            .call_method1("getClassTypeId", ())
            .unwrap()
            .call_method1(
                "isDerivedFrom",
                (coin
                    .getattr("SoEvent")
                    .unwrap()
                    .call_method0("getClassTypeId")
                    .unwrap(),),
            )
            .unwrap()
            .extract::<bool>()
            .unwrap());
    });
}
