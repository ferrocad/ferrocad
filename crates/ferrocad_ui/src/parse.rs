//! The XML reader.
//!
//! The reader walks the `<ui>` document in order and builds the arena in
//! [`crate::model`]. It is deliberately strict: an element, layout class,
//! widget class or property-value form it does not model becomes a
//! [`UiError`], never a silently dropped field.

use roxmltree::{Document, Node, NodeType};

use crate::error::{UiError, UiErrorKind};
use crate::model::{
    Attribute, Color, ComboItem, Connection, ConnectionHint, CustomWidget, Form, IconSet,
    IconSetVariant, Layout, LayoutDefault, LayoutItem, LayoutItemKind, Point, Property,
    PropertyValue, Rect, Resource, Size, SizePolicy, Spacer, SpacerOrientation, TabPage, Widget,
    WidgetId,
};

/// Qt widget classes this reader accepts in a `class` attribute.
///
/// The first block is the measured Draft inventory (`qt-strategy.md` §2.3); the
/// rest are common Designer widgets kept so a new form does not need a parser
/// change. `Gui::*` classes are always accepted because the upstream tree has at
/// least one undeclared custom class (`preferences-svg.ui` uses
/// `Gui::PrefSpinBox` without a `<customwidget>` entry).
const KNOWN_QT_WIDGET_CLASSES: &[&str] = &[
    // Measured inventory (17 plain Qt classes, 326 instances).
    "QCheckBox",
    "QComboBox",
    "QDialog",
    "QDialogButtonBox",
    "QDoubleSpinBox",
    "QFontComboBox",
    "QFrame",
    "QGroupBox",
    "QLabel",
    "QLineEdit",
    "QPushButton",
    "QRadioButton",
    "QScrollArea",
    "QSpinBox",
    "QTabWidget",
    "QTreeView",
    "QWidget",
    // Common Designer widgets not used by the Draft forms.
    "QCalendarWidget",
    "QDateEdit",
    "QDateTimeEdit",
    "QDial",
    "QGraphicsView",
    "QLCDNumber",
    "QListView",
    "QListWidget",
    "QMainWindow",
    "QMenu",
    "QMenuBar",
    "QPlainTextEdit",
    "QProgressBar",
    "QScrollBar",
    "QSlider",
    "QSplitter",
    "QStackedWidget",
    "QStatusBar",
    "QTableView",
    "QTableWidget",
    "QTextBrowser",
    "QTextEdit",
    "QTimeEdit",
    "QToolBar",
    "QToolButton",
    "QTreeWidget",
];

/// Qt layout classes this reader accepts in a `<layout class="...">`.
const KNOWN_QT_LAYOUT_CLASSES: &[&str] = &[
    "QFormLayout",
    "QGridLayout",
    "QHBoxLayout",
    "QStackedLayout",
    "QVBoxLayout",
];

pub(crate) fn parse_form(file: &str, source: &str) -> Result<Form, UiError> {
    // `roxmltree` recurses while building deeply nested documents, so the depth
    // limit has to be enforced on the source before its tree exists. Without
    // this, a 300-level document overflows the stack before a typed error is
    // possible.
    if let Some(offset) = first_element_past_depth(source, crate::MAX_DEPTH) {
        return Err(UiError::at_offset(
            file,
            source,
            offset,
            UiErrorKind::NestingTooDeep {
                limit: crate::MAX_DEPTH,
            },
        ));
    }
    let document = Document::parse(source).map_err(|error| {
        let pos = error.pos();
        UiError::at_line_col(
            file,
            pos.row,
            pos.col,
            None,
            UiErrorKind::Xml {
                message: error.to_string(),
            },
        )
    })?;
    let root = document.root_element();
    let parser = Parser {
        file,
        source,
        form: empty_form(),
    };
    if root.tag_name().name() != "ui" {
        return Err(parser.unexpected(root, "document", "<ui>"));
    }
    parser.parse_ui(root)
}

fn empty_form() -> Form {
    Form {
        ui_version: String::new(),
        class: String::new(),
        root: 0,
        widgets: Vec::new(),
        layouts: Vec::new(),
        spacers: Vec::new(),
        custom_widgets: Vec::new(),
        resources: Vec::new(),
        connections: Vec::new(),
        tab_order: Vec::new(),
        layout_default: None,
    }
}

struct Parser<'a> {
    file: &'a str,
    source: &'a str,
    form: Form,
}

impl Parser<'_> {
    fn parse_ui(mut self, ui: Node<'_, '_>) -> Result<Form, UiError> {
        self.form.ui_version = self.required_attr(ui, "version")?;
        let children = element_children(ui);

        // `<customwidgets>` follows `<widget>` in every upstream file, but the
        // combo-item decision needs `extends`, so the registry is parsed first.
        let mut seen_custom_widgets = false;
        for child in &children {
            if child.tag_name().name() == "customwidgets" {
                if seen_custom_widgets {
                    return Err(self.duplicate(*child, "customwidgets", "<ui>"));
                }
                seen_custom_widgets = true;
                self.parse_custom_widgets(*child)?;
            }
        }

        let mut seen_class = false;
        let mut seen_widget = false;
        let mut seen_resources = false;
        let mut seen_connections = false;
        let mut seen_tabstops = false;

        for child in &children {
            match child.tag_name().name() {
                "class" => {
                    if seen_class {
                        return Err(self.duplicate(*child, "class", "<ui>"));
                    }
                    seen_class = true;
                    self.form.class = self.non_empty_text(*child)?;
                }
                "widget" => {
                    if seen_widget {
                        return Err(self.duplicate(*child, "widget", "<ui>"));
                    }
                    seen_widget = true;
                    let root = self.parse_widget(*child, None, None, 0)?;
                    self.form.root = root;
                }
                "customwidgets" => {}
                "resources" => {
                    if seen_resources {
                        return Err(self.duplicate(*child, "resources", "<ui>"));
                    }
                    seen_resources = true;
                    self.parse_resources(*child)?;
                }
                "connections" => {
                    if seen_connections {
                        return Err(self.duplicate(*child, "connections", "<ui>"));
                    }
                    seen_connections = true;
                    self.parse_connections(*child)?;
                }
                "tabstops" => {
                    if seen_tabstops {
                        return Err(self.duplicate(*child, "tabstops", "<ui>"));
                    }
                    seen_tabstops = true;
                    self.parse_tabstops(*child)?;
                }
                "layoutdefault" => self.parse_layout_default(*child)?,
                _ => {
                    return Err(self.unexpected(
                        *child,
                        "<ui>",
                        "<class>, <widget>, <customwidgets>, <resources>, <connections>, \
                         <tabstops> or <layoutdefault>",
                    ));
                }
            }
        }

        if !seen_class {
            return Err(self.error(
                ui,
                UiErrorKind::MissingElement {
                    parent: "<ui>".to_owned(),
                    expected: "<class>".to_owned(),
                },
            ));
        }
        if !seen_widget {
            return Err(self.error(
                ui,
                UiErrorKind::MissingElement {
                    parent: "<ui>".to_owned(),
                    expected: "<widget>".to_owned(),
                },
            ));
        }

        Ok(self.form)
    }

    // -----------------------------------------------------------------------
    // Widgets
    // -----------------------------------------------------------------------

    fn parse_widget(
        &mut self,
        node: Node<'_, '_>,
        parent: Option<WidgetId>,
        owning_layout: Option<usize>,
        depth: usize,
    ) -> Result<WidgetId, UiError> {
        self.check_depth(node, depth)?;
        let class = self.required_attr(node, "class")?;
        let name = self.required_attr(node, "name")?;
        self.check_widget_class(node, &class)?;

        let id = self.form.widgets.len();
        self.form.widgets.push(Widget {
            class: class.clone(),
            name,
            parent,
            children: Vec::new(),
            owning_layout,
            layout: None,
            properties: Vec::new(),
            geometry: None,
            combo_items: Vec::new(),
            tab_pages: Vec::new(),
            attributes: Vec::new(),
        });
        if let Some(parent) = parent {
            self.form.widgets[parent].children.push(id);
        }

        let is_combo = self.is_combo(&class);
        let is_tab_widget = class == "QTabWidget";
        let expected = if is_combo {
            "property, layout, widget, item or attribute"
        } else {
            "property, layout, widget or attribute"
        };

        for child in element_children(node) {
            match child.tag_name().name() {
                "property" => {
                    let property = self.parse_property(child)?;
                    if property.name == "geometry" {
                        match &property.value {
                            PropertyValue::Rect(rect) => {
                                self.form.widgets[id].geometry = Some(*rect);
                            }
                            other => {
                                return Err(self.invalid_value(
                                    child,
                                    property_value_kind(other),
                                    "a <rect> for the geometry property",
                                ));
                            }
                        }
                    }
                    self.form.widgets[id].properties.push(property);
                }
                "layout" => {
                    if self.form.widgets[id].layout.is_some() {
                        return Err(self.duplicate(child, "layout", &describe(node)));
                    }
                    let layout = self.parse_layout(child, Some(id), depth + 1)?;
                    self.form.widgets[id].layout = Some(layout);
                }
                "widget" => {
                    let child_id = self.parse_widget(child, Some(id), None, depth + 1)?;
                    if is_tab_widget {
                        let title = self.form.widgets[child_id]
                            .attributes
                            .iter()
                            .find(|attribute| attribute.name == "title")
                            .map(|attribute| match &attribute.value {
                                PropertyValue::String(title) => Ok(title.clone()),
                                other => Err(self.invalid_value(
                                    child,
                                    property_value_kind(other),
                                    "a <string> title",
                                )),
                            })
                            .transpose()?
                            .ok_or_else(|| {
                                self.error(
                                    child,
                                    UiErrorKind::MissingElement {
                                        parent: describe(child),
                                        expected: "<attribute name=\"title\">".to_owned(),
                                    },
                                )
                            })?;
                        self.form.widgets[id].tab_pages.push(TabPage {
                            title,
                            widget: child_id,
                        });
                    }
                }
                "item" if is_combo => {
                    let item = self.parse_combo_item(child)?;
                    self.form.widgets[id].combo_items.push(item);
                }
                "attribute" => {
                    let attribute = self.parse_attribute(child)?;
                    self.form.widgets[id].attributes.push(attribute);
                }
                _ => return Err(self.unexpected(child, &describe(node), expected)),
            }
        }

        Ok(id)
    }

    fn check_widget_class(&self, node: Node<'_, '_>, class: &str) -> Result<(), UiError> {
        if class.starts_with("Gui::") || KNOWN_QT_WIDGET_CLASSES.contains(&class) {
            Ok(())
        } else {
            Err(self.error(
                node,
                UiErrorKind::UnknownClass {
                    class: class.to_owned(),
                    expected: "a known Qt widget class or a 'Gui::' custom widget class"
                        .to_owned(),
                },
            ))
        }
    }

    /// Whether this class takes `<item>` children and can therefore carry
    /// combo-box entries. `Gui::PrefComboBox` is listed explicitly because the
    /// upstream registry is incomplete in one file.
    fn is_combo(&self, class: &str) -> bool {
        if matches!(
            class,
            "QComboBox" | "QFontComboBox" | "Gui::PrefComboBox"
        ) {
            return true;
        }
        self.form.custom_widgets.iter().any(|custom| {
            custom.class == class
                && matches!(
                    custom.extends.as_deref(),
                    Some("QComboBox" | "QFontComboBox")
                )
        })
    }

    // -----------------------------------------------------------------------
    // Layouts and spacers
    // -----------------------------------------------------------------------

    fn parse_layout(
        &mut self,
        node: Node<'_, '_>,
        parent: Option<WidgetId>,
        depth: usize,
    ) -> Result<usize, UiError> {
        self.check_depth(node, depth)?;
        let class = self.required_attr(node, "class")?;
        if !KNOWN_QT_LAYOUT_CLASSES.contains(&class.as_str()) {
            return Err(self.error(
                node,
                UiErrorKind::UnknownClass {
                    class,
                    expected: "a known Qt layout class".to_owned(),
                },
            ));
        }
        let name = node.attribute("name").map(str::to_owned);
        let row_stretch = self.u32_list_attr(node, "rowstretch")?;
        let column_stretch = self.u32_list_attr(node, "columnstretch")?;
        let row_minimum_height = self.u32_list_attr(node, "rowminimumheight")?;
        let column_minimum_width = self.u32_list_attr(node, "columnminimumwidth")?;

        let id = self.form.layouts.len();
        self.form.layouts.push(Layout {
            class,
            name,
            parent,
            items: Vec::new(),
            properties: Vec::new(),
            row_stretch,
            column_stretch,
            row_minimum_height,
            column_minimum_width,
        });

        for child in element_children(node) {
            match child.tag_name().name() {
                "item" => {
                    let item = self.parse_layout_item(child, id, depth + 1)?;
                    self.form.layouts[id].items.push(item);
                }
                "property" => {
                    let property = self.parse_property(child)?;
                    self.form.layouts[id].properties.push(property);
                }
                _ => return Err(self.unexpected(child, &describe(node), "item or property")),
            }
        }

        Ok(id)
    }

    fn parse_layout_item(
        &mut self,
        node: Node<'_, '_>,
        layout: usize,
        depth: usize,
    ) -> Result<LayoutItem, UiError> {
        self.check_depth(node, depth)?;
        let row = self.optional_u32_attr(node, "row")?;
        let column = self.optional_u32_attr(node, "column")?;
        let row_span = self.optional_u32_attr(node, "rowspan")?.unwrap_or(1);
        let column_span = self.optional_u32_attr(node, "colspan")?.unwrap_or(1);

        let children = element_children(node);
        let child = match children.as_slice() {
            [] => {
                return Err(self.error(
                    node,
                    UiErrorKind::MissingElement {
                        parent: "<item>".to_owned(),
                        expected: "a <widget>, <layout> or <spacer>".to_owned(),
                    },
                ));
            }
            [child] => *child,
            [_, extra, ..] => {
                return Err(self.unexpected(
                    *extra,
                    "<item>",
                    "exactly one of <widget>, <layout> or <spacer>",
                ));
            }
        };

        let widget_parent = self.form.layouts[layout].parent;
        let kind = match child.tag_name().name() {
            "widget" => LayoutItemKind::Widget(self.parse_widget(
                child,
                widget_parent,
                Some(layout),
                depth + 1,
            )?),
            "layout" => {
                LayoutItemKind::Layout(self.parse_layout(child, widget_parent, depth + 1)?)
            }
            "spacer" => LayoutItemKind::Spacer(self.parse_spacer(child)?),
            _ => return Err(self.unexpected(child, "<item>", "widget, layout or spacer")),
        };

        Ok(LayoutItem {
            row,
            column,
            row_span,
            column_span,
            kind,
        })
    }

    fn parse_spacer(&mut self, node: Node<'_, '_>) -> Result<usize, UiError> {
        let name = self.required_attr(node, "name")?;
        let mut spacer = Spacer {
            name,
            orientation: None,
            size_hint: None,
            size_type: None,
            properties: Vec::new(),
        };

        for child in element_children(node) {
            if child.tag_name().name() != "property" {
                return Err(self.unexpected(child, "<spacer>", "property"));
            }
            let property = self.parse_property(child)?;
            match property.name.as_str() {
                "orientation" => {
                    spacer.orientation = Some(match &property.value {
                        PropertyValue::Enum(value) => match value.as_str() {
                            "Qt::Horizontal" => SpacerOrientation::Horizontal,
                            "Qt::Vertical" => SpacerOrientation::Vertical,
                            _ => {
                                return Err(self.invalid_value(
                                    child,
                                    value.clone(),
                                    "Qt::Horizontal or Qt::Vertical",
                                ));
                            }
                        },
                        other => {
                            return Err(self.invalid_value(
                                child,
                                property_value_kind(other),
                                "a Qt::Horizontal or Qt::Vertical <enum>",
                            ));
                        }
                    });
                }
                "sizeHint" => {
                    spacer.size_hint = Some(match &property.value {
                        PropertyValue::Size(size) => *size,
                        other => {
                            return Err(self.invalid_value(
                                child,
                                property_value_kind(other),
                                "a <size>",
                            ));
                        }
                    });
                }
                "sizeType" => {
                    spacer.size_type = Some(match &property.value {
                        PropertyValue::Enum(value) => value.clone(),
                        other => {
                            return Err(self.invalid_value(
                                child,
                                property_value_kind(other),
                                "an <enum>",
                            ));
                        }
                    });
                }
                _ => {}
            }
            spacer.properties.push(property);
        }

        let id = self.form.spacers.len();
        self.form.spacers.push(spacer);
        Ok(id)
    }

    // -----------------------------------------------------------------------
    // Values
    // -----------------------------------------------------------------------

    fn parse_property(&self, node: Node<'_, '_>) -> Result<Property, UiError> {
        let name = self.required_attr(node, "name")?;
        let std_set = match node.attribute("stdset") {
            None | Some("1") => true,
            Some("0") => false,
            Some(other) => {
                return Err(self.error(
                    node,
                    UiErrorKind::InvalidAttribute {
                        element: "property".to_owned(),
                        attribute: "stdset".to_owned(),
                        value: other.to_owned(),
                        expected: "0 or 1".to_owned(),
                    },
                ));
            }
        };
        let value_node = self.single_value_child(node, "<property>")?;
        Ok(Property {
            name,
            std_set,
            value: self.parse_value(value_node)?,
        })
    }

    fn parse_attribute(&self, node: Node<'_, '_>) -> Result<Attribute, UiError> {
        let name = self.required_attr(node, "name")?;
        let value_node = self.single_value_child(node, "<attribute>")?;
        Ok(Attribute {
            name,
            value: self.parse_value(value_node)?,
        })
    }

    fn single_value_child<'a, 'input>(
        &self,
        node: Node<'a, 'input>,
        parent: &str,
    ) -> Result<Node<'a, 'input>, UiError>
    where
        'input: 'a,
    {
        let children = element_children(node);
        match children.as_slice() {
            [] => Err(self.error(
                node,
                UiErrorKind::MissingElement {
                    parent: parent.to_owned(),
                    expected: "a value element".to_owned(),
                },
            )),
            [child] => Ok(*child),
            [_, extra, ..] => Err(self.unexpected(*extra, parent, "exactly one value element")),
        }
    }

    fn parse_value(&self, node: Node<'_, '_>) -> Result<PropertyValue, UiError> {
        match node.tag_name().name() {
            "string" => Ok(PropertyValue::String(self.text_only(node)?)),
            "cstring" => Ok(PropertyValue::CString(self.text_only(node)?)),
            "number" => {
                let text = self.text_only(node)?;
                Ok(PropertyValue::Int(self.parse_i64(node, &text)?))
            }
            "double" => {
                let text = self.text_only(node)?;
                let value = text.trim().parse::<f64>().map_err(|_| {
                    self.invalid_value(node, &text, "a floating-point number")
                })?;
                Ok(PropertyValue::Double(value))
            }
            "bool" => {
                let text = self.text_only(node)?;
                match text.trim() {
                    "true" => Ok(PropertyValue::Bool(true)),
                    "false" => Ok(PropertyValue::Bool(false)),
                    _ => Err(self.invalid_value(node, &text, "true or false")),
                }
            }
            "enum" => Ok(PropertyValue::Enum(self.non_empty_text(node)?)),
            "set" => {
                let text = self.text_only(node)?;
                let values: Vec<String> = text
                    .split('|')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect();
                if values.is_empty() {
                    Err(self.invalid_value(node, &text, "a |-separated set value"))
                } else {
                    Ok(PropertyValue::Set(values))
                }
            }
            "stringlist" => {
                let mut values = Vec::new();
                for child in element_children(node) {
                    if child.tag_name().name() != "string" {
                        return Err(self.unexpected(child, "<stringlist>", "<string>"));
                    }
                    values.push(self.text_only(child)?);
                }
                Ok(PropertyValue::StringList(values))
            }
            "point" => Ok(PropertyValue::Point(Point {
                x: self.required_i32_child(node, "x")?,
                y: self.required_i32_child(node, "y")?,
            })),
            "size" => Ok(PropertyValue::Size(Size {
                width: self.required_i32_child(node, "width")?,
                height: self.required_i32_child(node, "height")?,
            })),
            "rect" => Ok(PropertyValue::Rect(Rect {
                x: self.required_i32_child(node, "x")?,
                y: self.required_i32_child(node, "y")?,
                width: self.required_i32_child(node, "width")?,
                height: self.required_i32_child(node, "height")?,
            })),
            "color" => Ok(PropertyValue::Color(Color {
                red: self.required_u8_child(node, "red")?,
                green: self.required_u8_child(node, "green")?,
                blue: self.required_u8_child(node, "blue")?,
                alpha: self.optional_u8_child(node, "alpha")?,
            })),
            "sizepolicy" => Ok(PropertyValue::SizePolicy(SizePolicy {
                horizontal: self.required_attr(node, "hsizetype")?,
                vertical: self.required_attr(node, "vsizetype")?,
                horizontal_stretch: self.required_u32_child(node, "horstretch")?,
                vertical_stretch: self.required_u32_child(node, "verstretch")?,
            })),
            "iconset" => {
                let theme = node.attribute("theme").map(str::to_owned);
                let mut variants = Vec::new();
                for child in element_children(node) {
                    let state = match child.tag_name().name() {
                        "normaloff" | "normalon" | "activeoff" | "activeon" | "disabledoff"
                        | "disabledon" | "selectedoff" | "selectedon" => {
                            child.tag_name().name().to_owned()
                        }
                        _ => {
                            return Err(self.unexpected(
                                child,
                                "<iconset>",
                                "normaloff, normalon, activeoff, activeon, disabledoff, \
                                 disabledon, selectedoff or selectedon",
                            ));
                        }
                    };
                    variants.push(IconSetVariant {
                        state,
                        file: self.text_only(child)?,
                    });
                }
                Ok(PropertyValue::IconSet(IconSet { theme, variants }))
            }
            _ => Err(self.error(
                node,
                UiErrorKind::UnsupportedValue {
                    element: node.tag_name().name().to_owned(),
                },
            )),
        }
    }

    fn parse_combo_item(&self, node: Node<'_, '_>) -> Result<ComboItem, UiError> {
        let mut item = ComboItem {
            text: String::new(),
            icon: None,
            properties: Vec::new(),
        };
        for child in element_children(node) {
            if child.tag_name().name() != "property" {
                return Err(self.unexpected(child, "<item>", "<property>"));
            }
            let property = self.parse_property(child)?;
            match property.name.as_str() {
                "text" => match &property.value {
                    PropertyValue::String(text) => item.text = text.clone(),
                    other => {
                        return Err(self.invalid_value(
                            child,
                            property_value_kind(other),
                            "a <string>",
                        ));
                    }
                },
                "icon" => match &property.value {
                    PropertyValue::IconSet(icon) => item.icon = Some(icon.clone()),
                    other => {
                        return Err(self.invalid_value(
                            child,
                            property_value_kind(other),
                            "an <iconset>",
                        ));
                    }
                },
                _ => {}
            }
            item.properties.push(property);
        }
        Ok(item)
    }

    // -----------------------------------------------------------------------
    // Registry, resources, connections and tab order
    // -----------------------------------------------------------------------

    fn parse_custom_widgets(&mut self, node: Node<'_, '_>) -> Result<(), UiError> {
        for child in element_children(node) {
            if child.tag_name().name() != "customwidget" {
                return Err(self.unexpected(child, "<customwidgets>", "<customwidget>"));
            }
            let class = self.required_child_text(child, "class")?;
            let extends = self.optional_child_text(child, "extends")?;
            let header = self.optional_child_text(child, "header")?;
            for grandchild in element_children(child) {
                match grandchild.tag_name().name() {
                    "class" | "extends" | "header" => {}
                    _ => {
                        return Err(self.unexpected(
                            grandchild,
                            &describe(child),
                            "<class>, <extends> or <header>",
                        ));
                    }
                }
            }
            self.form.custom_widgets.push(CustomWidget {
                class,
                extends,
                header,
            });
        }
        Ok(())
    }

    fn parse_resources(&mut self, node: Node<'_, '_>) -> Result<(), UiError> {
        for child in element_children(node) {
            if child.tag_name().name() != "include" {
                return Err(self.unexpected(child, "<resources>", "<include>"));
            }
            self.form.resources.push(Resource {
                location: self.required_attr(child, "location")?,
            });
        }
        Ok(())
    }

    fn parse_connections(&mut self, node: Node<'_, '_>) -> Result<(), UiError> {
        for child in element_children(node) {
            if child.tag_name().name() != "connection" {
                return Err(self.unexpected(child, "<connections>", "<connection>"));
            }
            let sender = self.required_child_text(child, "sender")?;
            let signal = self.required_child_text(child, "signal")?;
            let receiver = self.required_child_text(child, "receiver")?;
            let slot = self.required_child_text(child, "slot")?;
            let mut hints = Vec::new();

            for grandchild in element_children(child) {
                match grandchild.tag_name().name() {
                    "sender" | "signal" | "receiver" | "slot" => {}
                    "hints" => {
                        for hint in element_children(grandchild) {
                            if hint.tag_name().name() != "hint" {
                                return Err(self.unexpected(hint, "<hints>", "<hint>"));
                            }
                            let kind = self.required_attr(hint, "type")?;
                            hints.push(ConnectionHint {
                                kind,
                                x: self.optional_i32_child(hint, "x")?,
                                y: self.optional_i32_child(hint, "y")?,
                            });
                        }
                    }
                    _ => {
                        return Err(self.unexpected(
                            grandchild,
                            "<connection>",
                            "<sender>, <signal>, <receiver>, <slot> or <hints>",
                        ));
                    }
                }
            }

            self.form.connections.push(Connection {
                sender,
                signal,
                receiver,
                slot,
                hints,
            });
        }
        Ok(())
    }

    fn parse_tabstops(&mut self, node: Node<'_, '_>) -> Result<(), UiError> {
        for child in element_children(node) {
            if child.tag_name().name() != "tabstop" {
                return Err(self.unexpected(child, "<tabstops>", "<tabstop>"));
            }
            self.form.tab_order.push(self.non_empty_text(child)?);
        }
        Ok(())
    }

    fn parse_layout_default(&mut self, node: Node<'_, '_>) -> Result<(), UiError> {
        if self.form.layout_default.is_some() {
            return Err(self.duplicate(node, "layoutdefault", "<ui>"));
        }
        if let Some(child) = element_children(node).first() {
            return Err(self.unexpected(*child, "<layoutdefault>", "no child elements"));
        }
        self.form.layout_default = Some(LayoutDefault {
            margin: self.optional_u32_attr(node, "margin")?,
            spacing: self.optional_u32_attr(node, "spacing")?,
        });
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Small helpers
    // -----------------------------------------------------------------------

    fn check_depth(&self, node: Node<'_, '_>, depth: usize) -> Result<(), UiError> {
        if depth > crate::MAX_DEPTH {
            Err(self.error(
                node,
                UiErrorKind::NestingTooDeep {
                    limit: crate::MAX_DEPTH,
                },
            ))
        } else {
            Ok(())
        }
    }

    fn error(&self, node: Node<'_, '_>, kind: UiErrorKind) -> UiError {
        UiError::at_offset(self.file, self.source, node.range().start, kind)
    }

    fn unexpected(&self, node: Node<'_, '_>, parent: &str, expected: &str) -> UiError {
        self.error(
            node,
            UiErrorKind::UnexpectedElement {
                parent: parent.to_owned(),
                found: node.tag_name().name().to_owned(),
                expected: expected.to_owned(),
            },
        )
    }

    fn duplicate(&self, node: Node<'_, '_>, element: &str, parent: &str) -> UiError {
        self.error(
            node,
            UiErrorKind::Duplicate {
                element: element.to_owned(),
                parent: parent.to_owned(),
            },
        )
    }

    fn invalid_value(
        &self,
        node: Node<'_, '_>,
        value: impl Into<String>,
        expected: &str,
    ) -> UiError {
        self.error(
            node,
            UiErrorKind::InvalidValue {
                element: node.tag_name().name().to_owned(),
                value: value.into(),
                expected: expected.to_owned(),
            },
        )
    }

    fn required_attr(&self, node: Node<'_, '_>, attribute: &str) -> Result<String, UiError> {
        node.attribute(attribute).map(str::to_owned).ok_or_else(|| {
            self.error(
                node,
                UiErrorKind::MissingAttribute {
                    element: node.tag_name().name().to_owned(),
                    attribute: attribute.to_owned(),
                },
            )
        })
    }

    fn optional_u32_attr(
        &self,
        node: Node<'_, '_>,
        attribute: &str,
    ) -> Result<Option<u32>, UiError> {
        match node.attribute(attribute) {
            None => Ok(None),
            Some(value) => value
                .trim()
                .parse::<u32>()
                .map(Some)
                .map_err(|_| self.invalid_attribute(node, attribute, value, "a non-negative integer")),
        }
    }

    fn u32_list_attr(&self, node: Node<'_, '_>, attribute: &str) -> Result<Vec<u32>, UiError> {
        let Some(value) = node.attribute(attribute) else {
            return Ok(Vec::new());
        };
        if value.trim().is_empty() {
            return Ok(Vec::new());
        }
        value
            .split(',')
            .map(|part| {
                part.trim().parse::<u32>().map_err(|_| {
                    self.invalid_attribute(
                        node,
                        attribute,
                        value,
                        "a comma-separated list of non-negative integers",
                    )
                })
            })
            .collect()
    }

    fn invalid_attribute(
        &self,
        node: Node<'_, '_>,
        attribute: &str,
        value: &str,
        expected: &str,
    ) -> UiError {
        self.error(
            node,
            UiErrorKind::InvalidAttribute {
                element: node.tag_name().name().to_owned(),
                attribute: attribute.to_owned(),
                value: value.to_owned(),
                expected: expected.to_owned(),
            },
        )
    }

    fn text_only(&self, node: Node<'_, '_>) -> Result<String, UiError> {
        let mut text = String::new();
        for child in node.children() {
            match child.node_type() {
                NodeType::Text => text.push_str(child.text().unwrap_or("")),
                NodeType::Element => {
                    return Err(self.unexpected(child, &describe(node), "text only"));
                }
                NodeType::Comment | NodeType::PI | NodeType::Root => {}
            }
        }
        Ok(text)
    }

    fn non_empty_text(&self, node: Node<'_, '_>) -> Result<String, UiError> {
        let text = self.text_only(node)?;
        let trimmed = text.trim();
        if trimmed.is_empty() {
            Err(self.invalid_value(node, text, "non-empty text"))
        } else {
            Ok(trimmed.to_owned())
        }
    }

    fn find_unique_child<'a, 'input>(
        &self,
        node: Node<'a, 'input>,
        tag: &str,
    ) -> Result<Option<Node<'a, 'input>>, UiError>
    where
        'input: 'a,
    {
        let mut found = None;
        for child in element_children(node) {
            if child.tag_name().name() == tag {
                if found.is_some() {
                    return Err(self.duplicate(child, tag, &describe(node)));
                }
                found = Some(child);
            }
        }
        Ok(found)
    }

    fn required_child_text(&self, node: Node<'_, '_>, tag: &str) -> Result<String, UiError> {
        let child = self.find_unique_child(node, tag)?.ok_or_else(|| {
            self.error(
                node,
                UiErrorKind::MissingElement {
                    parent: describe(node),
                    expected: format!("<{tag}>"),
                },
            )
        })?;
        self.non_empty_text(child)
    }

    fn optional_child_text(
        &self,
        node: Node<'_, '_>,
        tag: &str,
    ) -> Result<Option<String>, UiError> {
        match self.find_unique_child(node, tag)? {
            None => Ok(None),
            Some(child) => self.non_empty_text(child).map(Some),
        }
    }

    fn required_i32_child(&self, node: Node<'_, '_>, tag: &str) -> Result<i32, UiError> {
        let child = self.required_text_child(node, tag)?;
        let text = self.text_only(child)?;
        text.trim()
            .parse::<i32>()
            .map_err(|_| self.invalid_value(child, &text, "an integer"))
    }

    fn required_u32_child(&self, node: Node<'_, '_>, tag: &str) -> Result<u32, UiError> {
        let child = self.required_text_child(node, tag)?;
        let text = self.text_only(child)?;
        text.trim()
            .parse::<u32>()
            .map_err(|_| self.invalid_value(child, &text, "a non-negative integer"))
    }

    fn required_u8_child(&self, node: Node<'_, '_>, tag: &str) -> Result<u8, UiError> {
        let child = self.required_text_child(node, tag)?;
        let text = self.text_only(child)?;
        text.trim()
            .parse::<u8>()
            .map_err(|_| self.invalid_value(child, &text, "an integer from 0 to 255"))
    }

    fn optional_u8_child(&self, node: Node<'_, '_>, tag: &str) -> Result<Option<u8>, UiError> {
        match self.find_unique_child(node, tag)? {
            None => Ok(None),
            Some(child) => {
                let text = self.text_only(child)?;
                text.trim()
                    .parse::<u8>()
                    .map(Some)
                    .map_err(|_| self.invalid_value(child, &text, "an integer from 0 to 255"))
            }
        }
    }

    fn optional_i32_child(&self, node: Node<'_, '_>, tag: &str) -> Result<Option<i32>, UiError> {
        match self.find_unique_child(node, tag)? {
            None => Ok(None),
            Some(child) => {
                let text = self.text_only(child)?;
                text.trim()
                    .parse::<i32>()
                    .map(Some)
                    .map_err(|_| self.invalid_value(child, &text, "an integer"))
            }
        }
    }

    fn required_text_child<'a, 'input>(
        &self,
        node: Node<'a, 'input>,
        tag: &str,
    ) -> Result<Node<'a, 'input>, UiError>
    where
        'input: 'a,
    {
        self.find_unique_child(node, tag)?.ok_or_else(|| {
            self.error(
                node,
                UiErrorKind::MissingElement {
                    parent: describe(node),
                    expected: format!("<{tag}>"),
                },
            )
        })
    }

    fn parse_i64(&self, node: Node<'_, '_>, text: &str) -> Result<i64, UiError> {
        text.trim()
            .parse::<i64>()
            .map_err(|_| self.invalid_value(node, text, "an integer"))
    }
}

fn element_children<'a, 'input>(node: Node<'a, 'input>) -> Vec<Node<'a, 'input>>
where
    'input: 'a,
{
    node.children()
        .filter(|child| child.is_element())
        .collect()
}

/// Return the offset of the first opening element that would exceed `limit`
/// elements of nesting.
///
/// This is a byte-level pre-scan, not an XML parser: it only has to find element
/// boundaries reliably enough to bound the work `roxmltree` may do recursively.
/// Comments, CDATA, processing instructions and declarations are skipped, and
/// quoted attribute values are respected so a `>` or `<` inside a string cannot
/// be mistaken for a tag boundary. Malformed input falls through to the real XML
/// parser, which reports the syntax error.
fn first_element_past_depth(source: &str, limit: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut index = 0usize;
    let mut depth = 0usize;

    while index < bytes.len() {
        if bytes[index] != b'<' {
            index += 1;
            continue;
        }
        let start = index;
        let rest = &source[start..];
        if rest.starts_with("<!--") {
            index = find_after(source, start + 4, "-->").unwrap_or(bytes.len());
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            index = find_after(source, start + 9, "]]>").unwrap_or(bytes.len());
            continue;
        }
        if rest.starts_with("<?") {
            index = find_after(source, start + 2, "?>").unwrap_or(bytes.len());
            continue;
        }
        if rest.starts_with("<!") {
            index = skip_declaration(source, start);
            continue;
        }
        if rest.starts_with("</") {
            depth = depth.saturating_sub(1);
            let end = find_tag_end(source, start + 2);
            if end == bytes.len() {
                break;
            }
            index = end + 1;
            continue;
        }

        let end = find_tag_end(source, start + 1);
        if end == bytes.len() {
            break;
        }
        if depth + 1 > limit {
            return Some(start);
        }
        let mut last = end;
        while last > start && bytes[last - 1].is_ascii_whitespace() {
            last -= 1;
        }
        if last == start + 1 || bytes[last - 1] != b'/' {
            depth += 1;
        }
        index = end + 1;
    }

    None
}

fn find_after(source: &str, from: usize, pattern: &str) -> Option<usize> {
    source[from..]
        .find(pattern)
        .map(|offset| from + offset + pattern.len())
}

fn find_tag_end(source: &str, from: usize) -> usize {
    let bytes = source.as_bytes();
    let mut index = from;
    let mut quote = None;
    while index < bytes.len() {
        let byte = bytes[index];
        match quote {
            Some(open) => {
                if byte == open {
                    quote = None;
                }
            }
            None => match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'>' => return index,
                _ => {}
            },
        }
        index += 1;
    }
    bytes.len()
}

fn skip_declaration(source: &str, start: usize) -> usize {
    let bytes = source.as_bytes();
    let mut index = start + 2;
    let mut brackets = 0usize;
    while index < bytes.len() {
        match bytes[index] {
            b'[' => brackets += 1,
            b']' => brackets = brackets.saturating_sub(1),
            b'>' if brackets == 0 => return index + 1,
            _ => {}
        }
        index += 1;
    }
    bytes.len()
}

fn describe(node: Node<'_, '_>) -> String {
    match node.tag_name().name() {
        "widget" => match (node.attribute("class"), node.attribute("name")) {
            (Some(class), Some(name)) => format!("<widget class=\"{class}\" name=\"{name}\">"),
            _ => "<widget>".to_owned(),
        },
        "layout" => match node.attribute("class") {
            Some(class) => format!("<layout class=\"{class}\">"),
            None => "<layout>".to_owned(),
        },
        "property" => match node.attribute("name") {
            Some(name) => format!("<property name=\"{name}\">"),
            None => "<property>".to_owned(),
        },
        "customwidget" => "<customwidget>".to_owned(),
        other => format!("<{other}>"),
    }
}

fn property_value_kind(value: &PropertyValue) -> &'static str {
    match value {
        PropertyValue::String(_) => "<string>",
        PropertyValue::CString(_) => "<cstring>",
        PropertyValue::Int(_) => "<number>",
        PropertyValue::Double(_) => "<double>",
        PropertyValue::Bool(_) => "<bool>",
        PropertyValue::Enum(_) => "<enum>",
        PropertyValue::StringList(_) => "<stringlist>",
        PropertyValue::Point(_) => "<point>",
        PropertyValue::Size(_) => "<size>",
        PropertyValue::Rect(_) => "<rect>",
        PropertyValue::Color(_) => "<color>",
        PropertyValue::SizePolicy(_) => "<sizepolicy>",
        PropertyValue::Set(_) => "<set>",
        PropertyValue::IconSet(_) => "<iconset>",
    }
}
