//! The headless proxy state built from a parsed [`ferrocad_ui::Form`].
//!
//! The proxy classes in [`crate::proxy`] all share one [`FormState`]. It is the
//! mutable counterpart of the immutable form tree: widget text, numeric values,
//! checked/enabled/visible flags, combo items and the per-signal callback lists.
//!
//! Nothing here knows about Qt beyond class names. A class whose member is not in
//! the modelled set is marked unsupported, and the proxy turns that into an
//! `AttributeError` naming the form, class, widget and member.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use ferrocad_ui::{Form, LayoutItemKind, PropertyValue, Widget};
use pyo3::exceptions::PyAttributeError;
use pyo3::prelude::*;

/// The callbacks connected to one signal.
pub(crate) type Callbacks = Rc<RefCell<Vec<Py<PyAny>>>>;

/// Signals exposed as attributes on every [`crate::WidgetProxy`].
///
/// The first five are the ones Q2a's panels use; the rest are the other names in
/// the measured Draft contract (§2.4 of `qt-strategy.md`). Connections live in
/// the proxy, not in Qt.
pub(crate) const SIGNALS: &[&str] = &[
    "clicked",
    "stateChanged",
    "valueChanged",
    "currentIndexChanged",
    "textChanged",
    "toggled",
    "checkStateChanged",
    "textEdited",
    "accepted",
    "rejected",
    "fileNameSelected",
    "fileNameChanged",
];

/// Whether `name` is one of the modelled signals.
pub(crate) fn is_signal(name: &str) -> bool {
    SIGNALS.contains(&name)
}

/// Whether a widget class models `text()`/`setText()`.
pub(crate) fn supports_text(class: &str) -> bool {
    matches!(
        class,
        "QLabel"
            | "QPushButton"
            | "QCheckBox"
            | "QRadioButton"
            | "QLineEdit"
            | "Gui::InputField"
            | "Gui::QuantitySpinBox"
    )
}

/// Whether a widget class models `title()`/`setTitle()`.
pub(crate) fn supports_title(class: &str) -> bool {
    class == "QGroupBox"
}

/// Whether a widget class models `value()`/`setValue()`.
pub(crate) fn supports_value(class: &str) -> bool {
    matches!(
        class,
        "QSpinBox" | "QDoubleSpinBox" | "Gui::QuantitySpinBox"
    )
}

/// Whether a widget class models `isChecked()`/`setChecked()`.
pub(crate) fn supports_checked(class: &str) -> bool {
    matches!(class, "QCheckBox" | "QRadioButton" | "QPushButton")
}

/// Whether a widget class models `currentIndex()`/`currentText()`.
pub(crate) fn supports_combo(class: &str) -> bool {
    matches!(class, "QComboBox" | "QFontComboBox")
}

/// Whether a widget class models the `rawValue` dynamic property.
pub(crate) fn supports_raw_value(class: &str) -> bool {
    matches!(class, "Gui::InputField" | "Gui::QuantitySpinBox")
}

/// Whether a widget class models the `color` dynamic property.
pub(crate) fn supports_color(class: &str) -> bool {
    class == "Gui::ColorButton"
}

/// Whether a widget class models `fileName()`/`setFileName()`.
pub(crate) fn supports_file_name(class: &str) -> bool {
    class == "Gui::FileChooser"
}

/// Whether a widget class has a numeric `minimum`/`maximum` range.
pub(crate) fn supports_range(class: &str) -> bool {
    matches!(
        class,
        "QSpinBox" | "QDoubleSpinBox" | "Gui::QuantitySpinBox"
    )
}

/// Whether a layout class places items in a grid.
pub(crate) fn is_grid_layout(class: &str) -> bool {
    class == "QGridLayout"
}

/// Whether a widget class is a dialog root (so `accept`/`reject` exist).
pub(crate) fn is_dialog_root(class: &str) -> bool {
    class == "QDialog"
}

/// One entry in a layout, mutable for `addWidget`/`takeAt` surgery.
#[derive(Debug, Clone)]
pub(crate) struct LayoutEntry {
    pub kind: LayoutEntryKind,
    /// Grid row, when the item came from a grid or was added with one.
    pub row: Option<u32>,
    /// Grid column, when the item came from a grid or was added with one.
    pub column: Option<u32>,
    pub row_span: u32,
    pub column_span: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum LayoutEntryKind {
    Widget(usize),
    Layout(usize),
    Spacer(usize),
}

/// The mutable counterpart of one [`ferrocad_ui::Layout`].
pub(crate) struct LayoutState {
    pub class: String,
    pub name: Option<String>,
    /// The widget that owns this layout (nested layouts share it).
    pub parent: Option<usize>,
    pub items: Vec<LayoutEntry>,
}

/// One widget's mutable proxy state.
pub(crate) struct WidgetState {
    pub class: String,
    pub name: String,
    pub parent: Option<usize>,
    pub children: Vec<usize>,

    pub supports_text: bool,
    pub supports_value: bool,
    pub supports_checked: bool,
    pub supports_combo: bool,
    pub supports_raw_value: bool,
    pub supports_color: bool,
    pub supports_file_name: bool,
    pub supports_range: bool,
    /// `QSpinBox` reports an integer `value()`; the double variants report a float.
    pub value_is_int: bool,
    /// The layout this widget owns (`layout()` in Qt).
    pub layout: Option<usize>,
    /// The layout this widget is an item of, if any.
    pub owning_layout: Option<usize>,

    pub text: Option<String>,
    pub title: Option<String>,
    pub value: f64,
    pub checked: bool,
    pub enabled: bool,
    pub visible: bool,

    pub items: Vec<String>,
    pub current_index: i64,
    /// The `.ui` `currentText` property, used when there are no items.
    pub current_text: String,
    /// Whether the combo box is editable (an unknown `setCurrentText` then sets
    /// the text instead of being ignored).
    pub editable: bool,

    /// The unit-agnostic `rawValue` dynamic property (`Gui::InputField`).
    /// Classes that also have `value()` share that storage instead.
    pub raw_value: f64,
    /// The `color` dynamic property as `(r, g, b, a)`.
    pub color: Option<(u8, u8, u8, u8)>,
    /// The `fileName` dynamic property.
    pub file_name: String,
    /// The inclusive numeric range for spin boxes.
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,

    /// Connected callbacks by signal name. Created lazily on the first
    /// `widget.<signal>` access, so an unused form allocates no signal maps.
    pub signals: BTreeMap<String, Callbacks>,
}

impl WidgetState {
    fn from_widget(widget: &Widget) -> Self {
        let class = widget.class.clone();
        let items: Vec<String> = widget
            .combo_items
            .iter()
            .map(|item| item.text.clone())
            .collect();

        let value_is_int = class == "QSpinBox";
        let mut value = property_number(widget, "value").unwrap_or(0.0);
        if value_is_int {
            value = value.trunc();
        }

        let supports_range = supports_range(&class);
        let (minimum, maximum) = if supports_range {
            let (default_min, default_max) = match class.as_str() {
                "QSpinBox" => (0.0, 99.0),
                "QDoubleSpinBox" => (0.0, 99.99),
                // FreeCAD's quantity widgets default to the full double range.
                _ => (f64::MIN, f64::MAX),
            };
            (
                Some(property_number(widget, "minimum").unwrap_or(default_min)),
                Some(property_number(widget, "maximum").unwrap_or(default_max)),
            )
        } else {
            (None, None)
        };
        // Qt clamps the value written by Designer into the range.
        value = clamp(value, minimum, maximum);

        let mut current_index: i64 = if items.is_empty() { -1 } else { 0 };
        if let Some(index) = property_number(widget, "currentIndex") {
            current_index = index as i64;
        }
        let current_text = property_string(widget, "currentText").unwrap_or_default();
        if current_index < 0 && !current_text.is_empty() {
            if let Some(index) = items.iter().position(|item| item == &current_text) {
                current_index = index as i64;
            }
        }

        let supports_raw_value = supports_raw_value(&class);
        let raw_value = if supports_raw_value {
            property_number(widget, "rawValue")
                .or_else(|| property_number(widget, "value"))
                .or_else(|| property_number(widget, "quantity"))
                .unwrap_or(0.0)
        } else {
            0.0
        };

        Self {
            supports_text: supports_text(&class),
            supports_value: supports_value(&class),
            supports_checked: supports_checked(&class),
            supports_combo: supports_combo(&class),
            supports_raw_value,
            supports_color: supports_color(&class),
            supports_file_name: supports_file_name(&class),
            supports_range,
            value_is_int,
            layout: widget.layout,
            owning_layout: widget.owning_layout,
            text: supports_text(&class).then(|| property_string(widget, "text").unwrap_or_default()),
            title: supports_title(&class)
                .then(|| property_string(widget, "title").unwrap_or_default()),
            value,
            checked: property_bool(widget, "checked", false),
            enabled: property_bool(widget, "enabled", true),
            visible: property_bool(widget, "visible", true),
            items,
            current_index,
            current_text,
            editable: property_bool(widget, "editable", false),
            raw_value,
            color: if supports_color(&class) {
                Some(property_color(widget, "color").unwrap_or((0, 0, 0, 255)))
            } else {
                None
            },
            file_name: if supports_file_name(&class) {
                property_string(widget, "fileName").unwrap_or_default()
            } else {
                String::new()
            },
            minimum,
            maximum,
            signals: BTreeMap::new(),
            class,
            name: widget.name.clone(),
            parent: widget.parent,
            children: widget.children.clone(),
        }
    }
}

/// The shared mutable state of one loaded form.
pub(crate) struct FormState {
    /// The `.ui` path or label, used in every error message.
    pub file: String,
    /// The form's `<class>` name.
    pub class: String,
    /// Arena index of the root widget (always `0` for a parsed form).
    pub root: usize,
    pub widgets: Vec<WidgetState>,
    /// The mutable layout tree, parallel to `ferrocad_ui::Form::layouts`.
    pub layouts: Vec<LayoutState>,
    /// First widget index by Qt object name.
    pub by_name: HashMap<String, usize>,
    pub window_title: String,
    /// `QDialog::Accepted` (1) after `accept()`, `QDialog::Rejected` (0)
    /// otherwise. `exec_()` returns this without running an event loop.
    pub result: i64,
}

impl FormState {
    /// The four-part error the strategy document requires: form, class, widget
    /// and member. Never returns `None` to hide a missing member.
    pub fn error(&self, widget: usize, member: &str) -> PyErr {
        let widget = &self.widgets[widget];
        PyAttributeError::new_err(format!(
            "{} '{}' has no attribute '{}' (form {})",
            widget.class, widget.name, member, self.file
        ))
    }

    /// The same four-part error for a layout: form, layout class, layout name
    /// and member.
    pub fn layout_error(&self, layout: usize, member: &str) -> PyErr {
        let entry = &self.layouts[layout];
        let name = match &entry.name {
            Some(name) => format!("'{name}'"),
            None => format!("<unnamed layout #{layout}>"),
        };
        PyAttributeError::new_err(format!(
            "{} {} has no attribute '{}' (form {})",
            entry.class, name, member, self.file
        ))
    }

    /// The lazily-created callback list for one signal.
    pub fn callbacks(&mut self, widget: usize, signal: &str) -> Callbacks {
        self.widgets[widget]
            .signals
            .entry(signal.to_owned())
            .or_default()
            .clone()
    }
}

pub(crate) fn clamp(value: f64, minimum: Option<f64>, maximum: Option<f64>) -> f64 {
    let mut value = value;
    if let Some(minimum) = minimum {
        if value < minimum {
            value = minimum;
        }
    }
    if let Some(maximum) = maximum {
        if value > maximum {
            value = maximum;
        }
    }
    value
}

/// Build the proxy state from a parsed form.
pub(crate) fn build_state(form: &Form, file: impl Into<String>) -> FormState {
    let mut widgets = Vec::with_capacity(form.widgets.len());
    let mut by_name = HashMap::with_capacity(form.widgets.len());
    for (index, widget) in form.widgets.iter().enumerate() {
        // Duplicate names resolve to the first declaration, matching
        // `Form::widget_by_name`.
        by_name.entry(widget.name.clone()).or_insert(index);
        widgets.push(WidgetState::from_widget(widget));
    }
    let layouts = form
        .layouts
        .iter()
        .map(|layout| LayoutState {
            class: layout.class.clone(),
            name: layout.name.clone(),
            parent: layout.parent,
            items: layout
                .items
                .iter()
                .map(|item| LayoutEntry {
                    kind: match item.kind {
                        LayoutItemKind::Widget(widget) => LayoutEntryKind::Widget(widget),
                        LayoutItemKind::Layout(layout) => LayoutEntryKind::Layout(layout),
                        LayoutItemKind::Spacer(spacer) => LayoutEntryKind::Spacer(spacer),
                    },
                    row: item.row,
                    column: item.column,
                    row_span: item.row_span,
                    column_span: item.column_span,
                })
                .collect(),
        })
        .collect();
    let window_title = property_string(form.root_widget(), "windowTitle").unwrap_or_default();
    FormState {
        file: file.into(),
        class: form.class.clone(),
        root: form.root,
        widgets,
        layouts,
        by_name,
        window_title,
        result: 0,
    }
}

fn property_string(widget: &Widget, name: &str) -> Option<String> {
    match widget.property(name) {
        Some(PropertyValue::String(value)) | Some(PropertyValue::CString(value)) => {
            Some(value.clone())
        }
        _ => None,
    }
}

fn property_bool(widget: &Widget, name: &str, default: bool) -> bool {
    match widget.property(name) {
        Some(PropertyValue::Bool(value)) => *value,
        _ => default,
    }
}

fn property_number(widget: &Widget, name: &str) -> Option<f64> {
    match widget.property(name) {
        Some(PropertyValue::Int(value)) => Some(*value as f64),
        Some(PropertyValue::Double(value)) => Some(*value),
        _ => None,
    }
}

fn property_color(widget: &Widget, name: &str) -> Option<(u8, u8, u8, u8)> {
    match widget.property(name) {
        Some(PropertyValue::Color(color)) => Some((
            color.red,
            color.green,
            color.blue,
            color.alpha.unwrap_or(255),
        )),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UI: &str = r#"
<ui version="4.0">
 <class>TestForm</class>
 <widget class="QWidget" name="Form">
  <property name="windowTitle"><string>Test</string></property>
  <layout class="QVBoxLayout" name="layout">
   <item><widget class="QLabel" name="label">
    <property name="text"><string>Hello</string></property>
   </widget></item>
   <item><widget class="QSpinBox" name="spin">
    <property name="minimum"><number>0</number></property>
    <property name="maximum"><number>10</number></property>
    <property name="value"><number>5</number></property>
   </widget></item>
   <item><widget class="QCheckBox" name="check">
    <property name="checked"><bool>true</bool></property>
   </widget></item>
   <item><widget class="QComboBox" name="combo">
    <item><property name="text"><string>One</string></property></item>
    <item><property name="text"><string>Two</string></property></item>
   </widget></item>
  </layout>
 </widget>
</ui>
"#;

    #[test]
    fn models_the_core_properties_from_the_ui_tree() {
        let form = Form::parse_str(UI).expect("inline form parses");
        let state = build_state(&form, "inline.ui");

        assert_eq!(state.class, "TestForm");
        assert_eq!(state.file, "inline.ui");
        assert_eq!(state.window_title, "Test");
        // The root plus four children, in document order.
        assert_eq!(state.widgets.len(), 5);

        let label = &state.widgets[state.by_name["label"]];
        assert!(label.supports_text);
        assert_eq!(label.text.as_deref(), Some("Hello"));
        assert!(!label.supports_value);

        let spin = &state.widgets[state.by_name["spin"]];
        assert!(spin.supports_value);
        assert!(spin.value_is_int);
        assert_eq!(spin.value, 5.0);
        assert!(!spin.supports_text);

        let check = &state.widgets[state.by_name["check"]];
        assert!(check.supports_checked);
        assert!(check.checked);

        let combo = &state.widgets[state.by_name["combo"]];
        assert!(combo.supports_combo);
        assert_eq!(combo.items, ["One", "Two"]);
        assert_eq!(combo.current_index, 0);
    }

    #[test]
    fn capabilities_are_per_class_not_global() {
        assert!(supports_text("QLabel"));
        assert!(supports_text("Gui::InputField"));
        assert!(!supports_text("QSpinBox"));
        assert!(supports_value("QDoubleSpinBox"));
        assert!(!supports_value("QLineEdit"));
        assert!(supports_checked("QPushButton"));
        assert!(!supports_checked("QLabel"));
        assert!(supports_combo("QFontComboBox"));
        assert!(!supports_combo("QLabel"));
        assert!(is_dialog_root("QDialog"));
        assert!(!is_dialog_root("QWidget"));
    }

    #[test]
    fn the_error_names_form_class_widget_and_member() {
        let form = Form::parse_str(UI).expect("inline form parses");
        let state = build_state(&form, "inline.ui");
        let message = state
            .error(state.by_name["spin"], "text")
            .to_string();
        assert!(message.contains("QSpinBox"), "{message}");
        assert!(message.contains("'spin'"), "{message}");
        assert!(message.contains("'text'"), "{message}");
        assert!(message.contains("inline.ui"), "{message}");
    }
}
