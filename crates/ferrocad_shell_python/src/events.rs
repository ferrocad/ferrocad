//! The `SoEventCallback` listener: forwards viewport events into Python.
//!
//! This is the native half of the shell's event seam. The shell holds a
//! `Box<dyn EventListener>` built by the installed `AppBackend`; the browser host
//! supplies a listener of its own (or none) without linking CPython.

use ferrocad_inventor::{Directives, Event, EventListener, Hit, Scene};

/// An [`EventListener`] that forwards pointer events to the Python
/// `SoEventCallback`s in the scene (the scripted counterpart of the Rust listeners).
///
/// It only delivers the event: because the extracted scene is a copy, a callback
/// does not yet change what is rendered, so it reports no scene change.
pub struct PythonEventCallbacks;

impl EventListener for PythonEventCallbacks {
    fn on_event(
        &mut self,
        event: &Event,
        hit: Option<&Hit>,
        _scene: &mut Scene,
        directives: &Directives,
    ) -> bool {
        let _ = crate::python::dispatch(directives, event, hit);
        false
    }
}
