//! Reusable `bite-gpui` widgets shared by the FerroCAD shell and the bite-gpui
//! book.
//!
//! * [`window_chrome::window_frame`] draws client-side decorations (title bar and
//!   resize grips) where the compositor does not.
//! * [`text_field::TextInput`] is a single-line editable field that emits
//!   [`ChangeEvent`] and [`SubmitEvent`].
//! * [`text_area::TextAreaState`] is a scrollable text area with a read-only
//!   prefix and an editable, possibly multi-line, tail (the console).
//! * [`text_buffer::TextBuffer`] is the shared editing model underneath both.

pub mod events;
pub mod text_area;
pub mod text_buffer;
pub mod text_field;
pub mod window_chrome;

pub use events::{ChangeEvent, SubmitEvent};
pub use text_area::TextAreaState;
pub use text_buffer::TextBuffer;
pub use text_field::TextInput;
pub use window_chrome::window_frame;
