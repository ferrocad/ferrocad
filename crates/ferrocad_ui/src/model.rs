//! The typed form tree.
//!
//! A [`Form`] is an arena of widgets, layouts and spacers. Arena indices are
//! stable within the form and every collection is in document order, so two
//! parses of the same source produce identical results. There are no hash maps
//! in the model; name lookups are explicit linear helpers ([`Form::widget_by_name`])
//! so iteration order can never leak into the public API.

use std::collections::BTreeMap;

/// Index into [`Form::widgets`].
pub type WidgetId = usize;
/// Index into [`Form::layouts`].
pub type LayoutId = usize;
/// Index into [`Form::spacers`].
pub type SpacerId = usize;

/// A parsed Qt Designer form.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    /// The `version` attribute of `<ui>` (for example `"4.0"`).
    pub ui_version: String,
    /// The `<class>` element: the form's C++ class name.
    pub class: String,
    /// Index of the root `<widget>` in [`Form::widgets`]. Always `0` for a form
    /// returned by this crate.
    pub root: WidgetId,
    /// Every `<widget>` in document order (pre-order), including the root.
    pub widgets: Vec<Widget>,
    /// Every `<layout>` in document order (pre-order).
    pub layouts: Vec<Layout>,
    /// Every `<spacer>` in document order.
    pub spacers: Vec<Spacer>,
    /// The `<customwidgets>` registry, in document order.
    pub custom_widgets: Vec<CustomWidget>,
    /// The `<resources><include>` entries, in document order.
    pub resources: Vec<Resource>,
    /// The `<connections>` entries, in document order. They are stored but not
    /// executed; Q2 installs the dispatch.
    pub connections: Vec<Connection>,
    /// The `<tabstops><tabstop>` names, in document order.
    pub tab_order: Vec<String>,
    /// The optional `<layoutdefault>` element.
    pub layout_default: Option<LayoutDefault>,
}

impl Form {
    /// Parse `source`, using `<memory>` as the file name in errors.
    pub fn parse_str(source: &str) -> Result<Self, crate::UiError> {
        crate::parse::parse_form("<memory>", source)
    }

    /// Parse `source`, using `file` in error messages.
    pub fn parse_named(source: &str, file: impl Into<String>) -> Result<Self, crate::UiError> {
        let file = file.into();
        crate::parse::parse_form(&file, source)
    }

    /// Read and parse a `.ui` file. I/O failures are returned as
    /// [`crate::UiErrorKind::Io`] at `1:1`.
    pub fn from_path(path: impl AsRef<std::path::Path>) -> Result<Self, crate::UiError> {
        let path = path.as_ref();
        let file = path.display().to_string();
        let source = std::fs::read_to_string(path).map_err(|error| {
            crate::UiError::at_line_col(
                &file,
                1,
                1,
                None,
                crate::UiErrorKind::Io {
                    message: error.to_string(),
                },
            )
        })?;
        crate::parse::parse_form(&file, &source)
    }

    /// The root widget (always present on a successfully parsed form).
    pub fn root_widget(&self) -> &Widget {
        // `root` is only produced by the parser and always points into `widgets`.
        &self.widgets[self.root]
    }

    /// Look a widget up by its Qt object name.
    ///
    /// This is a linear scan in document order; duplicate names resolve to the
    /// first occurrence. The parser does not hide duplicates (upstream forms use
    /// unique names, and Q2 should decide whether that is an error).
    pub fn widget_by_name(&self, name: &str) -> Option<&Widget> {
        self.widgets.iter().find(|widget| widget.name == name)
    }

    /// The arena index of the first widget with this Qt object name.
    pub fn widget_id_by_name(&self, name: &str) -> Option<WidgetId> {
        self.widgets.iter().position(|widget| widget.name == name)
    }

    /// The custom-widget registry entry for a class, if one was declared.
    ///
    /// Note that upstream `preferences-svg.ui` uses `Gui::PrefSpinBox` without
    /// declaring it, so a widget class does not guarantee a registry entry.
    pub fn custom_widget(&self, class: &str) -> Option<&CustomWidget> {
        self.custom_widgets
            .iter()
            .find(|custom| custom.class == class)
    }

    /// A deterministic class histogram (`BTreeMap`), useful for drift checks.
    pub fn class_counts(&self) -> BTreeMap<&str, usize> {
        let mut counts = BTreeMap::new();
        for widget in &self.widgets {
            *counts.entry(widget.class.as_str()).or_insert(0) += 1;
        }
        counts
    }
}

/// One `<widget>` element.
#[derive(Debug, Clone, PartialEq)]
pub struct Widget {
    /// Qt class name, for example `QLabel` or `Gui::InputField`.
    pub class: String,
    /// Qt object name (`name` attribute).
    pub name: String,
    /// Nearest containing widget. `None` for the root.
    pub parent: Option<WidgetId>,
    /// Direct child widgets in document order, whether they appear directly or
    /// through a layout item. Tab pages and scroll-area contents are children.
    pub children: Vec<WidgetId>,
    /// The layout whose item contains this widget, if any.
    pub owning_layout: Option<LayoutId>,
    /// The layout owned by this widget (`<layout>` as a direct child), if any.
    pub layout: Option<LayoutId>,
    /// Properties in document order. `geometry` is kept here too, as well as in
    /// [`Widget::geometry`].
    pub properties: Vec<Property>,
    /// Convenience copy of the `geometry` property when it is a `<rect>`.
    pub geometry: Option<Rect>,
    /// Direct `<item>` children of a combo box, in document order.
    pub combo_items: Vec<ComboItem>,
    /// Pages of a `QTabWidget`, in document order.
    pub tab_pages: Vec<TabPage>,
    /// `<attribute>` elements (`title` for tab pages), in document order.
    pub attributes: Vec<Attribute>,
}

impl Widget {
    /// The typed value of the first property with this Qt name.
    pub fn property(&self, name: &str) -> Option<&PropertyValue> {
        self.properties
            .iter()
            .find(|property| property.name == name)
            .map(|property| &property.value)
    }

    /// The string value of a property, if it is a `<string>`.
    pub fn string_property(&self, name: &str) -> Option<&str> {
        match self.property(name)? {
            PropertyValue::String(value) => Some(value),
            _ => None,
        }
    }

    /// The `text` property, if it is a `<string>`.
    pub fn text(&self) -> Option<&str> {
        self.string_property("text")
    }

    /// The `title` property, if it is a `<string>`.
    pub fn title(&self) -> Option<&str> {
        self.string_property("title")
    }

    /// The `currentText` property, if it is a `<string>`.
    pub fn current_text(&self) -> Option<&str> {
        self.string_property("currentText")
    }
}

/// One `<layout>` element.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// Qt layout class (`QGridLayout`, `QHBoxLayout`, ...).
    pub class: String,
    /// Qt object name, when present.
    pub name: Option<String>,
    /// The widget that owns this layout. Nested layouts share the widget of
    /// their containing layout.
    pub parent: Option<WidgetId>,
    /// Items in document order.
    pub items: Vec<LayoutItem>,
    /// Properties of the layout itself (`margin`, `spacing`, ...).
    pub properties: Vec<Property>,
    /// `rowstretch` attribute, one entry per row.
    pub row_stretch: Vec<u32>,
    /// `columnstretch` attribute, one entry per column.
    pub column_stretch: Vec<u32>,
    /// `rowminimumheight` attribute, one entry per row.
    pub row_minimum_height: Vec<u32>,
    /// `columnminimumwidth` attribute, one entry per column.
    pub column_minimum_width: Vec<u32>,
}

/// One `<item>` inside a layout.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutItem {
    /// Grid row (`row` attribute), when present.
    pub row: Option<u32>,
    /// Grid column (`column` attribute), when present.
    pub column: Option<u32>,
    /// `rowspan`, defaulting to 1.
    pub row_span: u32,
    /// `colspan`, defaulting to 1.
    pub column_span: u32,
    /// What the item contains.
    pub kind: LayoutItemKind,
}

/// The three things a layout item may hold.
#[derive(Debug, Clone, PartialEq)]
pub enum LayoutItemKind {
    Widget(WidgetId),
    Layout(LayoutId),
    Spacer(SpacerId),
}

/// One `<spacer>` element.
#[derive(Debug, Clone, PartialEq)]
pub struct Spacer {
    /// Qt object name.
    pub name: String,
    /// The `orientation` property, when it is a `Qt::Horizontal`/`Qt::Vertical` enum.
    pub orientation: Option<SpacerOrientation>,
    /// The `sizeHint` property, when it is a `<size>`.
    pub size_hint: Option<Size>,
    /// The `sizeType` property (for example `QSizePolicy::Expanding`).
    pub size_type: Option<String>,
    /// All spacer properties in document order.
    pub properties: Vec<Property>,
}

/// Orientation carried by a spacer's `orientation` property.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpacerOrientation {
    Horizontal,
    Vertical,
}

/// A `<customwidget>` registry entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomWidget {
    /// The `Gui::` class name used by `<widget class="...">`.
    pub class: String,
    /// The `extends` base class, when declared.
    pub extends: Option<String>,
    /// The `header` include, when declared.
    pub header: Option<String>,
}

/// One item of a combo box.
#[derive(Debug, Clone, PartialEq)]
pub struct ComboItem {
    /// The `text` property (empty when the item has no text).
    pub text: String,
    /// The `icon` property, when it is an `<iconset>`.
    pub icon: Option<IconSet>,
    /// All item properties in document order.
    pub properties: Vec<Property>,
}

/// One page of a `QTabWidget`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabPage {
    /// The page's `title` attribute.
    pub title: String,
    /// The page widget (also a direct child of the tab widget).
    pub widget: WidgetId,
}

/// A property as written by Qt Designer.
#[derive(Debug, Clone, PartialEq)]
pub struct Property {
    /// Qt property name, unchanged.
    pub name: String,
    /// `stdset` attribute; `true` when the attribute is absent (the Qt default).
    pub std_set: bool,
    /// The typed value.
    pub value: PropertyValue,
}

/// A typed `<property>`/`<attribute>` value.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
    /// `<string>` (text preserved exactly; `notr` is ignored).
    String(String),
    /// `<cstring>`.
    CString(String),
    /// `<number>`.
    Int(i64),
    /// `<double>`.
    Double(f64),
    /// `<bool>`.
    Bool(bool),
    /// `<enum>` (for example `Qt::Horizontal`).
    Enum(String),
    /// `<stringlist><string>…</string>…</stringlist>`.
    StringList(Vec<String>),
    /// `<point>`.
    Point(Point),
    /// `<size>`.
    Size(Size),
    /// `<rect>`.
    Rect(Rect),
    /// `<color>`.
    Color(Color),
    /// `<sizepolicy>`.
    SizePolicy(SizePolicy),
    /// `<set>` split on `|`.
    Set(Vec<String>),
    /// `<iconset>`.
    IconSet(IconSet),
}

/// A `<point>` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// A `<size>` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: i32,
    pub height: i32,
}

/// A `<rect>` value (`x`, `y`, `width`, `height`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// A `<color>` value. `alpha` is `None` when the element omits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: Option<u8>,
}

/// A `<sizepolicy>` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizePolicy {
    pub horizontal: String,
    pub vertical: String,
    pub horizontal_stretch: u32,
    pub vertical_stretch: u32,
}

/// An `<iconset>` value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconSet {
    /// The `theme` attribute, when present.
    pub theme: Option<String>,
    /// State entries (`normaloff`, `normalon`, …) in document order.
    pub variants: Vec<IconSetVariant>,
}

/// One state file of an iconset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconSetVariant {
    /// The state element name, for example `normaloff`.
    pub state: String,
    /// The file path.
    pub file: String,
}

/// An `<attribute>` element, used for tab titles.
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub value: PropertyValue,
}

/// A `<resources><include>` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    /// The `location` attribute (usually a `.qrc` path).
    pub location: String,
}

/// A `<connection>` entry. Stored, not executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub sender: String,
    pub signal: String,
    pub receiver: String,
    pub slot: String,
    /// Designer's label hints, in document order.
    pub hints: Vec<ConnectionHint>,
}

/// One `<hint>` of a connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionHint {
    /// `type` attribute (`sourcelabel` or `destinationlabel`).
    pub kind: String,
    pub x: Option<i32>,
    pub y: Option<i32>,
}

/// The optional `<layoutdefault>` element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutDefault {
    pub margin: Option<u32>,
    pub spacing: Option<u32>,
}
