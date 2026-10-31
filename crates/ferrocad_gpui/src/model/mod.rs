//! Shell data models: the entity/object map, the selection and the view request state.
//!
//! These are plain data with no gpui widget state: a host can use them to drive a
//! headless render or to build its own surface.

pub mod display;
pub mod selection;
pub mod view;
