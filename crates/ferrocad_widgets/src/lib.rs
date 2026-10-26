//! Reusable `bite-gpui` widgets shared by the FerroCAD shell and the bite-gpui
//! book.
//!
//! * [`accordion`] stacks titled sections and gives the bodies up to their headers
//!   when the column runs out of height.
//! * [`scroll`] adds the scrollbar gpui never paints: a tracked scroll area and a
//!   draggable thumb.
//! * [`color`] is the property editor's colour row: a swatch and a fixed palette.
//! * [`form`] is the form controls: a boolean toggle switch and an enum select box
//!   with its dropdown list.
//! * [`slider`] maps a bounded number onto a log (or linear) track for the
//!   property editor's range overlay.
//! * [`window_chrome::window_frame`] draws client-side decorations (title bar and
//!   resize grips) where the compositor does not.
//! * [`text_field::TextInput`] is a single-line editable field that emits
//!   [`ChangeEvent`] and [`SubmitEvent`].
//! * [`text_area::TextAreaState`] is a scrollable text area with a read-only
//!   prefix and an editable, possibly multi-line, tail (the console).
//! * [`text_buffer::TextBuffer`] is the shared editing model underneath both.
//! * [`menu`] is the domain-free popover row list.
//! * [`wheel`] is the radial marking menu: a dielectric torus, wedge hit-testing and
//!   the semantic tone arcs, plus the [`WheelMenu`] sidecar that owns the gesture
//!   (armed/latched, tap-vs-flick, hover, overlay and dismissal).

pub mod accordion;
pub mod color;
pub mod events;
pub mod form;
pub mod menu;
pub mod scroll;
pub mod slider;
pub mod text_area;
pub mod text_buffer;
pub mod text_field;
pub mod wheel;
pub mod window_chrome;

pub use accordion::{AccordionLayout, AccordionSection, AccordionState, accordion, resolve_layout};
pub use color::{normalize_hex, palette, parse_hex, swatch};
pub use events::{ChangeEvent, SubmitEvent};
pub use form::{choice_list, select_box, toggle};
pub use menu::{MenuInfoRow, MenuRow, menu};
pub use scroll::{scroll_area, scroll_thumb, thumb_bounds};
pub use slider::{fraction as slider_fraction, trigger as slider_trigger};
pub use text_area::TextAreaState;
pub use text_buffer::TextBuffer;
pub use text_field::TextInput;
pub use wheel::{
    WheelEvent, WheelHit, WheelMenu, WheelSlot, WheelTone, slot_at, slot_offset, wheel,
};
pub use window_chrome::window_frame;
