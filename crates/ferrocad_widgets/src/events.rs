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

/// Emitted when a field with a [`char_filter`](crate::TextInput::set_char_filter)
/// refuses a typed character.
///
/// The character is **not** inserted; the host can give it a meaning of its own (the
/// rotate field uses `X`/`Y`/`Z` to switch rings while the angle keeps the caret).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RejectedChar {
    pub character: char,
}
