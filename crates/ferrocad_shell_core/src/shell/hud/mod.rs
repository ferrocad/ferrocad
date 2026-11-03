//! The in-context HUD: the Move/Rotate form and its keyboard state.
//!
//! [`move_hud`] owns the form for the current gesture; [`pill`] owns only which field,
//! if any, the keyboard holds and the form's two switches.

pub mod move_hud;
pub mod pill;
