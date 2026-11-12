//! The shared form state, with PyO3's handles and error shape.
//!
//! The state machine itself is [`ferrocad_ui::state`] — the same code
//! `ferrocad_shell_pywasm`'s native module drives under RustPython — so a change to
//! what `setValue` clamps or which classes have which members is made once. This
//! module only binds the callback handle to `Py<PyAny>` and turns the four-part
//! message into an `AttributeError`.

pub(crate) use ferrocad_ui::state::{
    Found, LayoutEntry, LayoutEntryKind, LayoutState, Query, build_state as build_generic_state,
    clamp, class_query, descendants, find_descendant, find_in_form, is_dialog_root, is_grid_layout,
    is_signal, layout_contains, text_query,
};

use pyo3::exceptions::PyAttributeError;
use pyo3::prelude::*;

use ferrocad_ui::Form;

/// The connected callbacks of one signal, as this binding stores them.
pub(crate) type Callbacks = ferrocad_ui::state::Callbacks<Py<PyAny>>;

/// One loaded form's mutable state.
pub(crate) type FormState = ferrocad_ui::state::FormState<Py<PyAny>>;

/// One widget's mutable state.
pub(crate) type WidgetState = ferrocad_ui::state::WidgetState<Py<PyAny>>;

/// Build the proxy state from a parsed form.
pub(crate) fn build_state(form: &Form, file: impl Into<String>) -> FormState {
    build_generic_state(form, file)
}

/// The four-part `AttributeError` the strategy document requires: form, class,
/// widget and member. Never returns `None` to hide a missing member.
pub(crate) trait StateError {
    fn error(&self, widget: usize, member: &str) -> PyErr;
    fn layout_error(&self, layout: usize, member: &str) -> PyErr;
}

impl<C> StateError for ferrocad_ui::state::FormState<C> {
    fn error(&self, widget: usize, member: &str) -> PyErr {
        PyAttributeError::new_err(self.widget_message(widget, member))
    }

    fn layout_error(&self, layout: usize, member: &str) -> PyErr {
        PyAttributeError::new_err(self.layout_message(layout, member))
    }
}
