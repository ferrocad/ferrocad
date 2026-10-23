//! # `ferrocad_ui` — a Qt Designer `.ui` reader
//!
//! Q1 of the Qt-reduction plan: read upstream Qt Designer XML into a typed form
//! tree. This crate has no Qt, no Python and no rendering; it is the data layer
//! that Q2's Python proxies and Q3's gpui renderer build on.
//!
//! * [`Form`] owns arenas of [`Widget`], [`Layout`] and [`Spacer`] values. Every
//!   collection is in document order and the arena indices are stable, so the
//!   public order is deterministic (there are no hash maps in the model).
//! * Properties keep their Qt name and become a typed [`PropertyValue`]
//!   (`<string>`, `<number>`, `<bool>`, `<enum>`, `<stringlist>`, `<point>`,
//!   `<size>`, `<rect>`, plus `<cstring>`, `<double>`, `<set>`, `<color>`,
//!   `<sizepolicy>` and `<iconset>`).
//! * A parse failure is a [`UiError`] with the file, 1-based line/column and the
//!   element that was expected. Malformed XML is an error, never a panic.
//!
//! # Example
//!
//! ```
//! use ferrocad_ui::{Form, PropertyValue};
//!
//! let source = r#"
//! <ui version="4.0">
//!  <class>HelloForm</class>
//!  <widget class="QWidget" name="Form">
//!   <property name="windowTitle"><string>Hello</string></property>
//!   <layout class="QVBoxLayout" name="layout">
//!    <item><widget class="QPushButton" name="button">
//!     <property name="text"><string>Press me</string></property>
//!    </widget></item>
//!   </layout>
//!  </widget>
//! </ui>
//! "#;
//!
//! let form = Form::parse_str(source)?;
//! assert_eq!(form.class, "HelloForm");
//! assert_eq!(form.widgets.len(), 2);
//! let button = form.widget_by_name("button").unwrap();
//! assert_eq!(button.text(), Some("Press me"));
//! assert_eq!(button.property("text"), Some(&PropertyValue::String("Press me".into())));
//! # Ok::<(), ferrocad_ui::UiError>(())
//! ```
//!
//! # Parsing coverage
//!
//! The reader accepts the structures the upstream Draft forms use: nested
//! `<layout>`/`<item>`/`<widget>`, `<property>` value elements, `<spacer>`,
//! `<customwidgets>`, `<resources>`, `<connections>` (stored, not executed),
//! `<tabstops>`, `<layoutdefault>`, combo-box `<item>` children, `QTabWidget`
//! pages and widget/layout `<attribute>` values.
//!
//! Deliberately not modelled: rendering, Qt object behaviour, signal dispatch,
//! `<action>`/`<actiongroup>`/`<addaction>`/`<zorder>`, `<layoutdefault>` child
//! elements, and property value forms outside the typed set above. An element or
//! class outside the accepted set fails the parse with its source position.
//!
//! # Relationship to the model
//!
//! Widgets know their nearest widget parent ([`Widget::parent`]), the layout that
//! contains them ([`Widget::owning_layout`]) and the layout they own
//! ([`Widget::layout`]). A layout's [`Layout::items`] carries the grid position,
//! span and nested widget/layout/spacer id, which is enough to rebuild the same
//! geometry in a renderer.

#![forbid(unsafe_code)]

mod error;
mod model;
mod parse;

pub use error::{UiError, UiErrorKind};
pub use model::{
    Attribute, Color, ComboItem, Connection, ConnectionHint, CustomWidget, Form, IconSet,
    IconSetVariant, Layout, LayoutDefault, LayoutId, LayoutItem, LayoutItemKind, Point, Property,
    PropertyValue, Rect, Resource, Size, SizePolicy, Spacer, SpacerId, SpacerOrientation, TabPage,
    Widget, WidgetId,
};

/// Maximum element nesting accepted by the reader.
///
/// A widget level costs three calls (widget → layout → item → widget), so the
/// usable widget depth is about a third of this number. The upstream Draft forms
/// nest at most 14 elements and 5 widgets deep, so 64 leaves ample headroom. The
/// limit is enforced on the source *before* the XML tree is built (see
/// [`UiErrorKind::NestingTooDeep`]), so malformed or hostile input cannot
/// exhaust the stack in the XML parser either.
pub const MAX_DEPTH: usize = 64;
