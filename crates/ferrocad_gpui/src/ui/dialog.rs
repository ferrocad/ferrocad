//! Dialog specs and value validation (A3).
//!
//! A command may return a dialog **spec** instead of acting directly: the facade
//! keeps the `on_accept` callback under a single-use token, and the shell renders
//! the spec as a modal. This module is the typed side of the spec plus the pure
//! validation the shell runs before it hands typed values back to Python.
//!
//! Nothing here touches gpui or Python: the modal state lives in [`crate::shell`],
//! and the token protocol in [`crate::commands`].

use serde::Deserialize;
use serde_json::Value;

use ferrocad_types::Quantity;

/// A modal a command asks the shell to show.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct DialogSpec {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub fields: Vec<DialogField>,
    /// The accept button's label.
    #[serde(default)]
    pub accept: String,
    /// The cancel button's label.
    #[serde(default)]
    pub cancel: String,
    /// How the shell presents the spec: `"modal"` (the default) or `"float"`.
    ///
    /// A floating dialog is a compact glass HUD with **no backdrop**, anchored beside
    /// the gizmo, so the canvas stays bright and the pointer keeps working.
    #[serde(default)]
    pub presentation: String,
    /// A live preview the shell drives while the dialog is edited.
    ///
    /// `"translation"` previews the `dx`/`dy`/`dz` fields on the selection without
    /// touching the document. `"workplane_offset"` is the `Offset Workplane…` HUD:
    /// the shell drives its one field from the active plane's `+N` stem, and the two
    /// end together. Empty means no preview.
    #[serde(default)]
    pub preview: String,
}

impl DialogSpec {
    pub fn accept_label(&self) -> &str {
        if self.accept.is_empty() {
            "OK"
        } else {
            &self.accept
        }
    }

    pub fn cancel_label(&self) -> &str {
        if self.cancel.is_empty() {
            "Cancel"
        } else {
            &self.cancel
        }
    }

    /// Whether the shell should float the dialog instead of centring it over a
    /// dimmed backdrop.
    pub fn is_floating(&self) -> bool {
        self.presentation == "float"
    }

    /// Whether the spec asks for a live translation preview.
    pub fn previews_translation(&self) -> bool {
        self.preview == "translation"
    }

    /// Whether this is the `Offset Workplane…` HUD, the typed half of the workplane
    /// stem's session (`ferrocad_stage::WORKPLANE_OFFSET_ID`).
    ///
    /// It rides in [`preview`](Self::preview), which the Python facade already forwards
    /// with the rest of the spec; a separate key would have to be added to the facade's
    /// whitelist first.
    pub fn drives_workplane_offset(&self) -> bool {
        self.preview == ferrocad_stage::WORKPLANE_OFFSET_ID
    }
}

/// One editable field.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct DialogField {
    /// The key in the values dict the callback receives.
    pub name: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub kind: String,
    /// The seed value; a string, number or bool depending on the kind.
    #[serde(default)]
    pub value: Option<Value>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    /// The enum's options.
    #[serde(default)]
    pub values: Vec<String>,
    /// A text field that may not be empty.
    #[serde(default)]
    pub required: bool,
}

impl DialogField {
    /// The label, falling back to the field name.
    pub fn label(&self) -> &str {
        if self.label.is_empty() {
            &self.name
        } else {
            &self.label
        }
    }

    pub fn kind(&self) -> Kind {
        Kind::parse(&self.kind)
    }

    /// The seed as text (for a text-like field or an enum's value).
    pub fn seed_text(&self) -> String {
        match &self.value {
            Some(Value::String(text)) => text.clone(),
            Some(Value::Number(number)) => number.to_string(),
            Some(Value::Bool(flag)) => flag.to_string(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        }
    }

    pub fn seed_flag(&self) -> bool {
        matches!(self.value, Some(Value::Bool(true)))
    }

    pub fn seed_choice(&self) -> usize {
        let Some(Value::String(text)) = &self.value else {
            return 0;
        };
        self.values
            .iter()
            .position(|choice| choice == text)
            .unwrap_or(0)
    }
}

/// The field kinds the shell knows how to render and validate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Int,
    Float,
    Quantity,
    Bool,
    Enum,
    /// An unknown kind is reported on accept, not crashed on.
    Unknown,
}

impl Kind {
    pub fn parse(kind: &str) -> Self {
        match kind {
            "text" => Self::Text,
            "int" => Self::Int,
            "float" => Self::Float,
            "quantity" => Self::Quantity,
            "bool" => Self::Bool,
            "enum" => Self::Enum,
            _ => Self::Unknown,
        }
    }

    /// Whether the field is edited with a text input.
    pub fn is_textual(self) -> bool {
        matches!(self, Self::Text | Self::Int | Self::Float | Self::Quantity)
    }
}

/// What the modal collected for one field.
#[derive(Clone, Debug, PartialEq)]
pub enum ValueState {
    /// The text in the input (text, int, float, quantity).
    Text(String),
    /// A bool toggle.
    Flag(bool),
    /// The selected enum option.
    Choice(usize),
}

impl ValueState {
    /// The seed state for a field.
    pub fn for_field(field: &DialogField) -> Self {
        match field.kind() {
            Kind::Bool => Self::Flag(field.seed_flag()),
            Kind::Enum => Self::Choice(field.seed_choice()),
            _ => Self::Text(field.seed_text()),
        }
    }
}

/// Validate the collected inputs and build the typed values dict the callback sees.
///
/// The values are JSON scalars (`string`/`number`/`bool`); Python receives them as
/// `str`/`float`/`int`/`bool`. A quantity is sent as its canonical millimetre value.
pub fn collect(spec: &DialogSpec, states: &[ValueState]) -> Result<Value, String> {
    if spec.fields.len() != states.len() {
        return Err("the dialog's fields changed".to_string());
    }
    let mut values = serde_json::Map::with_capacity(spec.fields.len());
    for (field, state) in spec.fields.iter().zip(states) {
        let value = match field.kind() {
            Kind::Text => {
                let text = text_of(state);
                if field.required && text.trim().is_empty() {
                    return Err(format!("{} is required", field.label()));
                }
                Value::String(text)
            }
            Kind::Int => {
                let number: i64 = text_of(state)
                    .trim()
                    .parse()
                    .map_err(|_| format!("{} must be a whole number", field.label()))?;
                check_range(field, number as f64)?;
                Value::from(number)
            }
            Kind::Float => {
                let number: f64 = text_of(state)
                    .trim()
                    .parse()
                    .map_err(|_| format!("{} must be a number", field.label()))?;
                check_range(field, number)?;
                Value::from(number)
            }
            Kind::Quantity => {
                let quantity = Quantity::parse(text_of(state).trim())
                    .map_err(|error| format!("{}: {error}", field.label()))?;
                let number = quantity.canonical_value();
                check_range(field, number)?;
                Value::from(number)
            }
            Kind::Bool => Value::Bool(match state {
                ValueState::Flag(flag) => *flag,
                _ => false,
            }),
            Kind::Enum => {
                let index = match state {
                    ValueState::Choice(index) => *index,
                    _ => 0,
                };
                let choice = field
                    .values
                    .get(index)
                    .ok_or_else(|| format!("{} has no option {}", field.label(), index + 1))?;
                Value::String(choice.clone())
            }
            Kind::Unknown => {
                return Err(format!(
                    "{} has an unsupported kind {:?}",
                    field.label(),
                    field.kind
                ));
            }
        };
        values.insert(field.name.clone(), value);
    }
    Ok(Value::Object(values))
}

fn text_of(state: &ValueState) -> String {
    match state {
        ValueState::Text(text) => text.clone(),
        _ => String::new(),
    }
}

/// The live translation a `"translation"` preview should show.
///
/// Text that is not a number yet (an empty field, a lone `-`, a half-typed unit)
/// contributes zero: a preview must never block or flash an error while typing.
/// `None` when the spec does not ask for a translation preview.
pub fn translation_preview(spec: &DialogSpec, states: &[ValueState]) -> Option<[f64; 3]> {
    if !spec.previews_translation() {
        return None;
    }
    let mut delta = [0.0; 3];
    for (field, state) in spec.fields.iter().zip(states) {
        let slot = match field.name.as_str() {
            "dx" => 0,
            "dy" => 1,
            "dz" => 2,
            _ => continue,
        };
        let ValueState::Text(text) = state else {
            continue;
        };
        let text = text.trim();
        delta[slot] = match field.kind() {
            Kind::Quantity => Quantity::parse(text).ok().map(|q| q.canonical_value()),
            Kind::Float => text.parse().ok(),
            Kind::Int => text.parse::<i64>().ok().map(|n| n as f64),
            _ => None,
        }
        .unwrap_or(0.0);
    }
    Some(delta)
}

fn check_range(field: &DialogField, value: f64) -> Result<(), String> {
    if let Some(min) = field.min {
        if value < min {
            return Err(format!("{} must be at least {min}", field.label()));
        }
    }
    if let Some(max) = field.max {
        if value > max {
            return Err(format!("{} must be at most {max}", field.label()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> DialogSpec {
        serde_json::from_value(serde_json::json!({
            "title": "Create Slab",
            "accept": "Create",
            "fields": [
                {"name": "Width", "label": "Width", "kind": "quantity", "value": 40.0, "min": 0.1},
                {"name": "Count", "label": "Count", "kind": "int", "value": 2, "min": 1, "max": 9},
                {"name": "Mode", "kind": "enum", "values": ["fast", "safe"], "value": "safe"},
                {"name": "Solid", "kind": "bool", "value": true},
                {"name": "Note", "kind": "text", "value": "hi", "required": true},
            ]
        }))
        .expect("a dialog spec")
    }

    #[test]
    fn a_spec_seeds_each_field_by_kind() {
        let spec = spec();
        assert_eq!(spec.accept_label(), "Create");
        assert_eq!(spec.cancel_label(), "Cancel");
        let states: Vec<ValueState> = spec.fields.iter().map(ValueState::for_field).collect();
        assert_eq!(
            states,
            vec![
                ValueState::Text("40.0".to_string()),
                ValueState::Text("2".to_string()),
                ValueState::Choice(1),
                ValueState::Flag(true),
                ValueState::Text("hi".to_string()),
            ]
        );
    }

    #[test]
    fn collect_produces_typed_values() {
        let spec = spec();
        let states = vec![
            ValueState::Text("25 mm".to_string()),
            ValueState::Text("3".to_string()),
            ValueState::Choice(0),
            ValueState::Flag(false),
            ValueState::Text("ok".to_string()),
        ];
        let values = collect(&spec, &states).expect("valid");
        assert_eq!(
            values,
            serde_json::json!({
                "Width": 25.0, "Count": 3, "Mode": "fast", "Solid": false, "Note": "ok"
            })
        );
    }

    #[test]
    fn a_bad_number_names_the_field() {
        let spec = spec();
        let mut states = vec![
            ValueState::Text("40".to_string()),
            ValueState::Text("3".to_string()),
            ValueState::Choice(0),
            ValueState::Flag(false),
            ValueState::Text("ok".to_string()),
        ];
        states[1] = ValueState::Text("many".to_string());
        let error = collect(&spec, &states).expect_err("not a number");
        assert!(error.contains("Count"), "{error}");

        states[1] = ValueState::Text("2.5".to_string());
        assert!(collect(&spec, &states).is_err(), "int is integral");
    }

    #[test]
    fn ranges_and_required_text_are_checked() {
        let spec = spec();
        let mut states = vec![
            ValueState::Text("0.05".to_string()),
            ValueState::Text("3".to_string()),
            ValueState::Choice(0),
            ValueState::Flag(false),
            ValueState::Text("ok".to_string()),
        ];
        let error = collect(&spec, &states).expect_err("below the minimum");
        assert!(error.contains("at least 0.1"), "{error}");

        states[0] = ValueState::Text("40".to_string());
        states[1] = ValueState::Text("10".to_string());
        let error = collect(&spec, &states).expect_err("above the maximum");
        assert!(error.contains("at most 9"), "{error}");

        states[1] = ValueState::Text("3".to_string());
        states[4] = ValueState::Text("   ".to_string());
        let error = collect(&spec, &states).expect_err("required text");
        assert!(error.contains("Note is required"), "{error}");
    }

    #[test]
    fn an_unknown_kind_is_reported_not_ignored() {
        let spec: DialogSpec = serde_json::from_value(serde_json::json!({
            "fields": [{"name": "Odd", "kind": "colour"}]
        }))
        .unwrap();
        let error = collect(&spec, &[ValueState::Text(String::new())]).expect_err("unknown kind");
        assert!(error.contains("unsupported kind"), "{error}");
    }

    #[test]
    fn presentation_and_preview_are_opt_in() {
        let plain: DialogSpec = serde_json::from_value(serde_json::json!({
            "fields": [{"name": "dx", "kind": "quantity"}]
        }))
        .unwrap();
        assert!(!plain.is_floating(), "the default stays modal");
        assert!(!plain.previews_translation());

        let hud: DialogSpec = serde_json::from_value(serde_json::json!({
            "presentation": "float",
            "preview": "translation",
            "fields": [
                {"name": "dx", "kind": "quantity"},
                {"name": "dy", "kind": "quantity"},
                {"name": "dz", "kind": "quantity"},
            ]
        }))
        .unwrap();
        assert!(hud.is_floating());
        assert!(hud.previews_translation());
    }

    #[test]
    fn a_translation_preview_parses_what_it_can_and_zeroes_the_rest() {
        let spec: DialogSpec = serde_json::from_value(serde_json::json!({
            "preview": "translation",
            "fields": [
                {"name": "dx", "kind": "quantity"},
                {"name": "dy", "kind": "quantity"},
                {"name": "dz", "kind": "quantity"},
            ]
        }))
        .unwrap();
        let states = vec![
            ValueState::Text("25 mm".to_string()),
            ValueState::Text("-".to_string()),
            ValueState::Text(String::new()),
        ];
        assert_eq!(translation_preview(&spec, &states), Some([25.0, 0.0, 0.0]));

        let plain = DialogSpec::default();
        assert_eq!(translation_preview(&plain, &states), None);
    }
}
