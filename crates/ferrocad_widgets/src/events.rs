//! Events emitted by the editable widgets.

/// Emitted when the user presses `Enter`.
#[derive(Clone, Debug)]
pub struct SubmitEvent {
    pub text: String,
}

/// Emitted after every edit. A controlled parent mirrors this to keep its value
/// in step with the field.
#[derive(Clone, Debug)]
pub struct ChangeEvent {
    pub text: String,
}
