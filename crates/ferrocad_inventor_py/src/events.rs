//! The `SoEvent`/`SoEventCallback` surface and the dispatch bridge.
//!
//! Workbenches register input callbacks on a `SoEventCallback` node——
//! `coin.SoMouseButtonEvent.getClassTypeId()` plus a Python callable — and the host
//! dispatches each pick through them. This module provides the Coin event classes
//! (`SoEvent` → `SoButtonEvent` → `SoMouseButtonEvent`, and `SoLocation2Event`),
//! the `SoPickedPoint`/`SoDetail` value objects, the `SoEventCallback` node, and
//! [`dispatch_event`], which the host calls with a picked [`Hit`].
//!
//! Deferred (see `docs/viewport/inventor-plan.md` §8): the global thread-local context Coin
//! uses (the context here lives on the node for the duration of a call), the
//! 1-based sub-shape `part_index` (needs topological naming), and the g-buffer pass.
//! Because the extracted [`Scene`] is a copy, a callback that edits Python nodes
//! does not (yet) change what is rendered.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use ferrocad_inventor::{Directives, Event, Hit, NodeKind, Topology};
use pyo3::prelude::*;

use crate::{SoNode, SoType, derives_from, node_initializer};

/// Coin's `SoButtonEvent.DOWN`.
const STATE_DOWN: i32 = 1;
/// Coin's `SoButtonEvent.UP`.
const STATE_UP: i32 = 0;

// ---------------------------------------------------------------------------
// Event classes
// ---------------------------------------------------------------------------

/// Base input event (Coin `SoEvent`); carries the device-pixel position and time.
#[pyclass(subclass)]
pub struct SoEvent {
    position: [i32; 2],
    time: f64,
}

impl SoEvent {
    fn initializer(position: [i32; 2], time: f64) -> PyClassInitializer<SoEvent> {
        PyClassInitializer::from(SoEvent { position, time })
    }
}

#[pymethods]
impl SoEvent {
    /// The time the event was created, in seconds since the process started.
    #[pyo3(name = "getTime")]
    fn get_time(&self) -> f64 {
        self.time
    }

    /// The device-pixel position (bottom-left origin), as `(x, y)`.
    #[pyo3(name = "getPosition")]
    fn get_position(&self) -> (i32, i32) {
        (self.position[0], self.position[1])
    }

    #[staticmethod]
    #[pyo3(name = "getClassTypeId")]
    fn get_class_type_id() -> SoType {
        SoType::named("SoEvent")
    }
}

/// A button event (Coin `SoButtonEvent`); adds the press/release state.
#[pyclass(extends = SoEvent, subclass)]
pub struct SoButtonEvent {
    state: i32,
}

#[pymethods]
impl SoButtonEvent {
    /// The button state: [`SoButtonEvent.DOWN`] or [`SoButtonEvent.UP`].
    #[pyo3(name = "getState")]
    fn get_state(&self) -> i32 {
        self.state
    }

    #[staticmethod]
    #[pyo3(name = "getClassTypeId")]
    fn get_class_type_id() -> SoType {
        SoType::named("SoButtonEvent")
    }

    /// The button is released (`SoButtonEvent.UP`).
    #[classattr]
    const UP: i32 = STATE_UP;
    /// The button is pressed (`SoButtonEvent.DOWN`).
    #[classattr]
    const DOWN: i32 = STATE_DOWN;
}

/// A mouse-button event (Coin `SoMouseButtonEvent`); adds the button number.
#[pyclass(extends = SoButtonEvent)]
pub struct SoMouseButtonEvent {
    button: i32,
}

#[pymethods]
impl SoMouseButtonEvent {
    /// The button (`SoMouseButtonEvent.BUTTON1` …), 0 when unknown.
    #[pyo3(name = "getButton")]
    fn get_button(&self) -> i32 {
        self.button
    }

    #[staticmethod]
    #[pyo3(name = "getClassTypeId")]
    fn get_class_type_id() -> SoType {
        SoType::named("SoMouseButtonEvent")
    }

    /// The primary button.
    #[classattr]
    const BUTTON1: i32 = 1;
    /// The middle button.
    #[classattr]
    const BUTTON2: i32 = 2;
    /// The secondary button.
    #[classattr]
    const BUTTON3: i32 = 3;
    /// Any button.
    #[classattr]
    const ANY: i32 = 0;
}

/// A pointer-move event (Coin `SoLocation2Event`).
#[pyclass(extends = SoEvent)]
pub struct SoLocation2Event;

#[pymethods]
impl SoLocation2Event {
    #[staticmethod]
    #[pyo3(name = "getClassTypeId")]
    fn get_class_type_id() -> SoType {
        SoType::named("SoLocation2Event")
    }
}

// ---------------------------------------------------------------------------
// Detail classes and SoPickedPoint
// ---------------------------------------------------------------------------

/// Base class for pick details (Coin `SoDetail`).
#[pyclass(subclass)]
pub struct SoDetail;

/// Detail for a face hit (Coin `SoFaceDetail`).
#[pyclass(extends = SoDetail)]
pub struct SoFaceDetail {
    face_index: i32,
    part_index: i32,
    point: [f32; 3],
}

#[pymethods]
impl SoFaceDetail {
    /// The 0-based face index within the shape.
    #[pyo3(name = "getFaceIndex")]
    fn get_face_index(&self) -> i32 {
        self.face_index
    }

    /// The 0-based part index within the face.
    ///
    /// Coin/FreeCAD would return the 1-based sub-shape part; that needs topological
    /// naming and is deferred, so this is always 0 for now.
    #[pyo3(name = "getPartIndex")]
    fn get_part_index(&self) -> i32 {
        self.part_index
    }

    /// A point on the face (the hit point, in world space).
    #[pyo3(name = "getPoint")]
    fn get_point(&self) -> (f32, f32, f32) {
        (self.point[0], self.point[1], self.point[2])
    }
}

/// Detail for a line hit (Coin `SoLineDetail`).
#[pyclass(extends = SoDetail)]
pub struct SoLineDetail {
    line_index: i32,
    part_index: i32,
}

#[pymethods]
impl SoLineDetail {
    /// The 0-based line index within the shape.
    #[pyo3(name = "getLineIndex")]
    fn get_line_index(&self) -> i32 {
        self.line_index
    }

    /// The 0-based part index within the line.
    #[pyo3(name = "getPartIndex")]
    fn get_part_index(&self) -> i32 {
        self.part_index
    }
}

/// A picked point (Coin `SoPickedPoint`): where the ray hit, its normal and detail.
#[pyclass]
pub struct SoPickedPoint {
    point: [f32; 3],
    normal: [f32; 3],
    detail: Py<PyAny>,
}

#[pymethods]
impl SoPickedPoint {
    /// The world-space intersection point.
    #[pyo3(name = "getPoint")]
    fn get_point(&self) -> (f32, f32, f32) {
        (self.point[0], self.point[1], self.point[2])
    }

    /// The world-space surface normal.
    #[pyo3(name = "getNormal")]
    fn get_normal(&self) -> (f32, f32, f32) {
        (self.normal[0], self.normal[1], self.normal[2])
    }

    /// The detail object (`SoFaceDetail`/`SoLineDetail`).
    #[pyo3(name = "getDetail")]
    fn get_detail(&self, py: Python<'_>) -> Py<PyAny> {
        self.detail.clone_ref(py)
    }
}

// ---------------------------------------------------------------------------
// SoEventCallback (the AST node) and its registry
// ---------------------------------------------------------------------------

/// Registered callbacks per `SoEventCallback` id. Strong refs: the node is in the
/// Python tree, and an arena/weakref identity is deferred.
static REGISTRY: LazyLock<Mutex<HashMap<u64, Py<PyAny>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Reacts to input events (Coin `SoEventCallback`).
///
/// A node (so it can sit in the scene graph); the compiler lifts it into a
/// [`Directive::EventCallback`](ferrocad_inventor::Directive) beside the draw queue,
/// and the host calls the registered Python
/// callables, with the current event and pick available via [`getEvent`](Self::get_event)
/// and [`getPickedPoint`](Self::get_picked_point).
#[pyclass(extends = SoNode)]
pub struct SoEventCallback {
    id: u64,
    callbacks: Vec<(String, Py<PyAny>)>,
    event: Option<Py<PyAny>>,
    picked_point: Option<Py<PyAny>>,
}

#[pymethods]
impl SoEventCallback {
    #[new]
    pub fn new() -> PyClassInitializer<Self> {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        node_initializer(NodeKind::EventCallback(id)).add_subclass(SoEventCallback {
            id,
            callbacks: Vec::new(),
            event: None,
            picked_point: None,
        })
    }

    /// `event_cb.addEventCallback(type, callback)` — register `callback` for events
    /// of `type` (or a subtype).
    #[pyo3(name = "addEventCallback")]
    fn add_event_callback(
        slf: &Bound<'_, Self>,
        event_type: &Bound<'_, SoType>,
        callback: Py<PyAny>,
    ) {
        let id = {
            let name = event_type.borrow().name().to_string();
            let mut this = slf.borrow_mut();
            this.callbacks.push((name, callback));
            this.id
        };
        REGISTRY
            .lock()
            .unwrap()
            .insert(id, slf.clone().into_any().unbind());
    }

    /// The event being dispatched, or `None` outside a dispatch.
    #[pyo3(name = "getEvent")]
    fn get_event(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        self.event.as_ref().map(|event| event.clone_ref(py))
    }

    /// The picked point being dispatched, or `None` outside a dispatch.
    #[pyo3(name = "getPickedPoint")]
    fn get_picked_point(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        self.picked_point.as_ref().map(|point| point.clone_ref(py))
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Dispatch `event` (with its `hit`, if any) to the active `SoEventCallback` nodes,
/// returning whether any callback was invoked.
///
/// The active callback ids travel in the neutral [`Directives`] the last compile
/// produced, so the host does not rescan the scene. The node carries the event and
/// pick for the duration of each call and clears them afterwards. A callback that
/// raises is reported and skipped, so one broken handler does not stop the rest.
pub fn dispatch_event(
    py: Python<'_>,
    directives: &Directives,
    event: &Event,
    hit: Option<&Hit>,
) -> PyResult<bool> {
    let ids: Vec<u64> = directives.event_callbacks().collect();
    if ids.is_empty() {
        return Ok(false);
    }
    let Some((py_event, class_name)) = build_event(py, event)? else {
        return Ok(false); // e.g. MouseLeave: Coin has no such event class
    };
    let py_pick = match hit {
        Some(hit) => Some(build_picked_point(py, hit)?),
        None => None,
    };

    let mut invoked = false;
    for id in ids {
        let node = REGISTRY
            .lock()
            .unwrap()
            .get(&id)
            .map(|node| node.clone_ref(py));
        let Some(node) = node else {
            continue;
        };

        // Install the context and snapshot the callbacks (dropping the borrow
        // before invoking, so a callback may read the node).
        let callbacks: Vec<(String, Py<PyAny>)> = {
            let bound = node.bind(py).downcast::<SoEventCallback>()?;
            let mut this = bound.borrow_mut();
            this.event = Some(py_event.clone_ref(py));
            this.picked_point = py_pick.as_ref().map(|point| point.clone_ref(py));
            this.callbacks
                .iter()
                .map(|(name, callback)| (name.clone(), callback.clone_ref(py)))
                .collect()
        };

        for (type_name, callback) in callbacks {
            if derives_from(class_name, &type_name) {
                invoked = true;
                if let Err(err) = callback.call1(py, (node.clone_ref(py),)) {
                    err.print(py);
                }
            }
        }

        // Clear the context: it lives only for the call.
        {
            let bound = node.bind(py).downcast::<SoEventCallback>()?;
            let mut this = bound.borrow_mut();
            this.event = None;
            this.picked_point = None;
        }
    }
    Ok(invoked)
}

/// Build the Python event object for `event`, with its class name.
fn build_event(py: Python<'_>, event: &Event) -> PyResult<Option<(Py<PyAny>, &'static str)>> {
    let time = elapsed_secs();
    let built = match *event {
        Event::MouseMove { position } => {
            let init = SoEvent::initializer(position, time).add_subclass(SoLocation2Event);
            (Py::new(py, init)?.into_any(), "SoLocation2Event")
        }
        Event::MouseButton {
            position,
            button,
            down,
        } => {
            let init = SoEvent::initializer(position, time)
                .add_subclass(SoButtonEvent {
                    state: if down { STATE_DOWN } else { STATE_UP },
                })
                .add_subclass(SoMouseButtonEvent {
                    button: i32::from(button),
                });
            (Py::new(py, init)?.into_any(), "SoMouseButtonEvent")
        }
        Event::MouseLeave => return Ok(None),
    };
    Ok(Some(built))
}

/// Build a `SoPickedPoint` (with an `SoFaceDetail` or `SoLineDetail` for `hit`).
fn build_picked_point(py: Python<'_>, hit: &Hit) -> PyResult<Py<PyAny>> {
    let detail: Py<PyAny> = match hit.topology {
        Topology::Lines => Py::new(
            py,
            PyClassInitializer::from(SoDetail).add_subclass(SoLineDetail {
                line_index: hit.element.map_or(-1, |element| element as i32),
                part_index: hit.part as i32,
            }),
        )?
        .into_any(),
        _ => Py::new(
            py,
            PyClassInitializer::from(SoDetail).add_subclass(SoFaceDetail {
                face_index: hit.element.map_or(-1, |element| element as i32),
                part_index: hit.part as i32,
                point: hit.point,
            }),
        )?
        .into_any(),
    };
    let picked = SoPickedPoint {
        point: hit.point,
        normal: hit.normal,
        detail,
    };
    Ok(Py::new(py, picked)?.into_any())
}

fn elapsed_secs() -> f64 {
    static START: LazyLock<Instant> = LazyLock::new(Instant::now);
    START.elapsed().as_secs_f64()
}

/// Register every class this module defines (called from the `coin` module).
pub(crate) fn register(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<SoEvent>()?;
    m.add_class::<SoButtonEvent>()?;
    m.add_class::<SoMouseButtonEvent>()?;
    m.add_class::<SoLocation2Event>()?;
    m.add_class::<SoDetail>()?;
    m.add_class::<SoFaceDetail>()?;
    m.add_class::<SoLineDetail>()?;
    m.add_class::<SoPickedPoint>()?;
    m.add_class::<SoEventCallback>()?;
    Ok(())
}
