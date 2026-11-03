//! Q3a: render a [`ferrocad_ui::Form`] as gpui elements.
//!
//! The upstream Draft panels are Qt Designer `.ui` files. Q1 turned them into a
//! typed form tree (`ferrocad_ui`); this module is the renderer. Given a parsed
//! [`Form`] it builds a gpui element tree with the same widget classes, the same
//! layout nesting (grid/vertical/horizontal, row/column/span) and the values the
//! form declares (text, title, value, checked, current index, enabled, visible).
//!
//! # The renderer API
//!
//! * [`FormDialog::new`] / [`FormDialog::from_path`] wrap a form in a dialog
//!   state. The type implements [`gpui::Render`], so an
//!   `Entity<FormDialog>` is the standalone test host: open it as a window root
//!   and draw.
//! * Every rendered widget carries a stable `debug_selector`. The form root is
//!   `form-<name>`; each widget is `form-<name>-<object-name>`. Tab strips are
//!   `form-<name>-<tab-widget>-tab-<index>`, and the dialog footer is
//!   `form-<name>-accept` / `form-<name>-reject`. The strings are interned once,
//!   so `VisualTestContext::debug_bounds` (which needs `&'static str`) can use
//!   them.
//! * [`FormDialog::accept`] / [`FormDialog::reject`] set a [`FormResult`] and
//!   close the dialog; [`FormDialog::close`] only hides it. [`FormDialog::show`]
//!   reopens it with a cleared result. The same three paths are wired to the
//!   footer buttons and the widget interactions below.
//! * Read-only accessors (`text_of`, `value_of`, `checked_of`,
//!   `current_index_of`, `is_enabled`, `is_visible`, `selector_of`, `kind_of`)
//!   exist for tests and for the Q3b shell wiring.
//!
//! # Widget classes mapped
//!
//! | Qt class | gpui rendering |
//! | --- | --- |
//! | `QLabel` | text |
//! | `QPushButton` | bordered button; clickable, checkable when the form says so |
//! | `QCheckBox` | `[x]`/`[ ]` toggle |
//! | `QRadioButton` | `(o)`/`( )`; clicking unchecks its siblings |
//! | `QLineEdit` | bordered text field (display-only in Q3a) |
//! | `QSpinBox` / `QDoubleSpinBox` | numeric field (display-only in Q3a) |
//! | `QComboBox` / `QFontComboBox` | current item + caret; click cycles the item |
//! | `QGroupBox` | titled, bordered group |
//! | `QWidget` / `QDialog` | layout container |
//! | `QFrame` | bordered container |
//! | `QScrollArea` | vertically scrolling container |
//! | `QTabWidget` | tab strip + the active page (pages use their own selectors) |
//! | `QDialogButtonBox` | labelled, non-interactive placeholder (the six core forms add their own footer buttons) |
//! | `Gui::InputField` | numeric field with the declared `unit` |
//! | `Gui::QuantitySpinBox` | numeric field with the declared `unit` |
//! | `Gui::ColorButton` | colour swatch when the form carries a `color`, otherwise a neutral swatch |
//! | `Gui::FileChooser` | "Choose file…" field |
//!
//! # Layouts
//!
//! `QGridLayout`/`QFormLayout` become a gpui grid using the item's `row`,
//! `column`, `rowspan` and `colspan`; `QHBoxLayout` becomes a flex row and
//! `QVBoxLayout`/`QStackedLayout` a flex column. Nested layouts recurse through
//! the model's layout items, and `<spacer>` elements become flexible or fixed
//! gaps. Uniform grid columns mean `columnstretch`/`rowstretch` and
//! `columnminimumwidth`/`rowminimumheight` are not reproduced yet (Q3b).
//!
//! # Deliberately not interactive
//!
//! Any class outside the table above (for example `QTreeView`) renders as a
//! labelled, bordered placeholder such as `[QTreeView]` with its stable
//! selector. It has no click handler and no hitbox: unsupported widgets are
//! never dressed up as working controls. The same rule applies to
//! `QDialogButtonBox`, which is a label rather than a button pair.
//!
//! Text entry, spin editing, combo item editing and the Python
//! `FreeCADGui.Control.showDialog`/`closeDialog` bridge are **Q3b**. This slice
//! shows the forms, reads their initial values and accepts/rejects them.
//!
//! # Shell wiring (Q3b)
//!
//! `shell.rs` holds an `Option<Entity<FormDialog>>` and renders it as an
//! absolute, full-window child above the workspace (the dialog draws its own
//! backdrop). [`FormDialog::accept`], [`FormDialog::reject`] and
//! [`FormDialog::close`] emit [`FormEvent::Closed`], so the shell learns about a
//! footer-button close as well as one driven by its own `Esc`/`Enter` handling.
//! The shell's `open_form`/`open_form_path`/`close_form` mirror
//! `FreeCADGui.Control.showDialog`/`closeDialog`; a form and a command dialog
//! never share the shell.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use gpui::{
    AnyElement, Context, Div, EventEmitter, IntoElement, Render, Window, div, prelude::*, px, rgb,
    rgba,
};

use ferrocad_ui::{Form, LayoutId, LayoutItem, LayoutItemKind, PropertyValue, Widget, WidgetId};

use crate::shell::{ACCENT, FG, MUTED, PANEL, PANEL_ALT};

/// What `accept`/`reject` left in the dialog.
///
/// `code` matches Qt's `QDialog::Accepted` (1) and `QDialog::Rejected` (0), so
/// Q3b can hand the same value to the Python proxy's `result()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormResult {
    Accepted,
    Rejected,
}

impl FormResult {
    /// `QDialog::Accepted` is 1, `QDialog::Rejected` is 0.
    pub fn code(self) -> i32 {
        match self {
            Self::Accepted => 1,
            Self::Rejected => 0,
        }
    }
}

/// What a [`FormDialog`] reports to its host when it closes.
///
/// `Closed(None)` is a plain [`FormDialog::close`] (`closeDialog`), which records
/// no result. A host that embeds the dialog listens so a footer-button click
/// releases the modal just like its own `Esc`/`Enter` handling would.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormEvent {
    Closed(Option<FormResult>),
}

impl EventEmitter<FormEvent> for FormDialog {}

/// The Qt classes the renderer knows how to draw.
///
/// [`WidgetKind::Placeholder`] is the catch-all for an unsupported class; it is
/// rendered as a label, never as a control.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetKind {
    Container,
    Label,
    PushButton,
    CheckBox,
    RadioButton,
    LineEdit,
    GroupBox,
    SpinBox,
    DoubleSpinBox,
    ComboBox,
    TabWidget,
    Frame,
    ScrollArea,
    DialogButtonBox,
    InputField,
    QuantitySpinBox,
    ColorButton,
    FileChooser,
    Placeholder,
}

impl WidgetKind {
    /// Map a Qt Designer `class` attribute to a renderer kind.
    ///
    /// The mapping is exhaustive for the six core Draft forms and the other
    /// draft forms Q5 will pick up. Anything unknown is a placeholder.
    pub fn of(class: &str) -> Self {
        match class {
            "QWidget" | "QDialog" => Self::Container,
            "QLabel" => Self::Label,
            "QPushButton" => Self::PushButton,
            "QCheckBox" => Self::CheckBox,
            "QRadioButton" => Self::RadioButton,
            "QLineEdit" => Self::LineEdit,
            "QGroupBox" => Self::GroupBox,
            "QSpinBox" => Self::SpinBox,
            "QDoubleSpinBox" => Self::DoubleSpinBox,
            "QComboBox" | "QFontComboBox" => Self::ComboBox,
            "QTabWidget" => Self::TabWidget,
            "QFrame" => Self::Frame,
            "QScrollArea" => Self::ScrollArea,
            "QDialogButtonBox" => Self::DialogButtonBox,
            "Gui::InputField" => Self::InputField,
            "Gui::QuantitySpinBox" => Self::QuantitySpinBox,
            "Gui::ColorButton" => Self::ColorButton,
            "Gui::FileChooser" => Self::FileChooser,
            _ => Self::Placeholder,
        }
    }

    /// Whether Q3a gives this kind an input handler.
    ///
    /// Display-only kinds still render their current value; they gain editing
    /// in Q3b.
    pub fn is_interactive(self) -> bool {
        matches!(
            self,
            Self::PushButton | Self::CheckBox | Self::RadioButton | Self::ComboBox | Self::TabWidget
        )
    }

    /// Whether the kind shows a number rather than text.
    pub fn is_numeric(self) -> bool {
        matches!(
            self,
            Self::SpinBox | Self::DoubleSpinBox | Self::InputField | Self::QuantitySpinBox
        )
    }
}

/// One widget's editable state, seeded from the form and updated by clicks.
#[derive(Debug, Clone)]
struct WidgetState {
    text: String,
    number: f64,
    checked: bool,
    index: usize,
    enabled: bool,
    visible: bool,
}

/// A parsed form plus the state a gpui dialog needs.
///
/// Create one with [`FormDialog::new`] or [`FormDialog::from_path`]. As an
/// `Entity<FormDialog>` it is both the standalone test host and the modal Q3b
/// will embed in the shell.
pub struct FormDialog {
    form: Form,
    name: String,
    title: String,
    open: bool,
    result: Option<FormResult>,
    states: Vec<WidgetState>,
    selectors: Vec<&'static str>,
    /// Tab-strip selectors, parallel to `form.widgets`; empty for non-tabs.
    tab_selectors: Vec<Vec<&'static str>>,
    panel_selector: &'static str,
    body_selector: &'static str,
    backdrop_selector: &'static str,
    accept_selector: &'static str,
    reject_selector: &'static str,
    accept_label: String,
    reject_label: String,
    /// The last push/check/radio/combo widget clicked, for tests and Q3b.
    last_action: Option<String>,
}

impl FormDialog {
    /// Wrap a parsed form. `name` seeds every `debug_selector`; the form's
    /// window title seeds the header (falling back to its class).
    pub fn new(form: Form, name: impl Into<String>) -> Self {
        let mut name = name.into();
        if name.is_empty() {
            name = form.class.clone();
        }
        let root = form.root_widget();
        let title = root
            .string_property("windowTitle")
            .or_else(|| root.title())
            .filter(|title| !title.is_empty())
            .unwrap_or(&form.class)
            .to_string();

        let selectors: Vec<&'static str> = form
            .widgets
            .iter()
            .enumerate()
            .map(|(id, widget)| {
                if id == form.root {
                    intern(format!("form-{name}"))
                } else {
                    intern(format!("form-{name}-{}", widget.name))
                }
            })
            .collect();
        let tab_selectors: Vec<Vec<&'static str>> = form
            .widgets
            .iter()
            .enumerate()
            .map(|(_, widget)| {
                (0..widget.tab_pages.len())
                    .map(|index| {
                        intern(format!("form-{name}-{}-tab-{index}", widget.name))
                    })
                    .collect()
            })
            .collect();
        let states = form
            .widgets
            .iter()
            .map(WidgetState::seed)
            .collect::<Vec<_>>();

        Self {
            panel_selector: intern(format!("form-{name}-panel")),
            body_selector: intern(format!("form-{name}-body")),
            backdrop_selector: intern(format!("form-{name}-backdrop")),
            accept_selector: intern(format!("form-{name}-accept")),
            reject_selector: intern(format!("form-{name}-reject")),
            selectors,
            tab_selectors,
            states,
            form,
            name,
            title,
            open: true,
            result: None,
            accept_label: "OK".to_string(),
            reject_label: "Cancel".to_string(),
            last_action: None,
        }
    }

    /// Read and parse a `.ui` file, then wrap it.
    pub fn from_path(
        path: impl AsRef<Path>,
        name: impl Into<String>,
    ) -> Result<Self, ferrocad_ui::UiError> {
        Ok(Self::new(Form::from_path(path)?, name))
    }

    /// Override the footer labels (the defaults are `OK` / `Cancel`).
    pub fn with_labels(mut self, accept: impl Into<String>, reject: impl Into<String>) -> Self {
        self.accept_label = accept.into();
        self.reject_label = reject.into();
        self
    }

    pub fn form(&self) -> &Form {
        &self.form
    }

    /// The selector stem passed to [`FormDialog::new`].
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn result(&self) -> Option<FormResult> {
        self.result
    }

    pub fn take_result(&mut self) -> Option<FormResult> {
        self.result.take()
    }

    /// The last push/check/radio/combo widget clicked.
    pub fn last_action(&self) -> Option<&str> {
        self.last_action.as_deref()
    }

    /// The selector for a widget name, or `None` when the form has no such widget.
    pub fn selector_of(&self, widget: &str) -> Option<&'static str> {
        self.id_of(widget).map(|id| self.selectors[id])
    }

    pub fn kind_of(&self, widget: &str) -> Option<WidgetKind> {
        self.id_of(widget)
            .map(|id| WidgetKind::of(&self.form.widgets[id].class))
    }

    /// The text the widget currently displays (for a spin/quantity field this
    /// includes the form's suffix or unit).
    pub fn text_of(&self, widget: &str) -> Option<String> {
        self.id_of(widget).map(|id| self.display_text(id))
    }

    /// A numeric widget's current value.
    pub fn value_of(&self, widget: &str) -> Option<f64> {
        let id = self.id_of(widget)?;
        self.kind_of_id(id).is_numeric().then(|| self.states[id].number)
    }

    pub fn checked_of(&self, widget: &str) -> Option<bool> {
        let id = self.id_of(widget)?;
        matches!(
            self.kind_of_id(id),
            WidgetKind::PushButton | WidgetKind::CheckBox | WidgetKind::RadioButton
        )
        .then(|| self.states[id].checked)
    }

    /// A combo box's current index or a tab widget's active page.
    pub fn current_index_of(&self, widget: &str) -> Option<usize> {
        let id = self.id_of(widget)?;
        matches!(self.kind_of_id(id), WidgetKind::ComboBox | WidgetKind::TabWidget)
            .then(|| self.states[id].index)
    }

    pub fn is_enabled(&self, widget: &str) -> Option<bool> {
        self.id_of(widget).map(|id| self.states[id].enabled)
    }

    pub fn is_visible(&self, widget: &str) -> Option<bool> {
        self.id_of(widget).map(|id| self.states[id].visible)
    }

    /// Set a text-like widget's display text.
    pub fn set_text(&mut self, widget: &str, text: impl Into<String>) -> bool {
        let Some(id) = self.id_of(widget) else {
            return false;
        };
        self.states[id].text = text.into();
        true
    }

    /// Set a numeric widget's value.
    pub fn set_value(&mut self, widget: &str, value: f64) -> bool {
        let Some(id) = self.id_of(widget) else {
            return false;
        };
        if !self.kind_of_id(id).is_numeric() {
            return false;
        }
        self.states[id].number = value;
        true
    }

    /// Set a check/radio/push button's checked state (radios are exclusive).
    pub fn set_checked(&mut self, widget: &str, checked: bool) -> bool {
        let Some(id) = self.id_of(widget) else {
            return false;
        };
        if !matches!(
            self.kind_of_id(id),
            WidgetKind::PushButton | WidgetKind::CheckBox | WidgetKind::RadioButton
        ) {
            return false;
        }
        self.states[id].checked = checked;
        if checked && matches!(self.kind_of_id(id), WidgetKind::RadioButton) {
            self.uncheck_siblings(id);
        }
        true
    }

    /// Set a combo box's current item or a tab widget's active page.
    pub fn set_current_index(&mut self, widget: &str, index: usize) -> bool {
        let Some(id) = self.id_of(widget) else {
            return false;
        };
        let limit = match self.kind_of_id(id) {
            WidgetKind::ComboBox => self.form.widgets[id].combo_items.len(),
            WidgetKind::TabWidget => self.form.widgets[id].tab_pages.len(),
            _ => return false,
        };
        if index >= limit {
            return false;
        }
        self.states[id].index = index;
        true
    }

    /// Show the dialog again with no result.
    pub fn show(&mut self, cx: &mut Context<Self>) {
        self.open = true;
        self.result = None;
        cx.notify();
    }

    /// Hide the dialog without recording a result.
    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        cx.notify();
        cx.emit(FormEvent::Closed(None));
    }

    /// Accept the form: record [`FormResult::Accepted`] and close.
    pub fn accept(&mut self, cx: &mut Context<Self>) {
        self.result = Some(FormResult::Accepted);
        self.open = false;
        cx.notify();
        cx.emit(FormEvent::Closed(self.result));
    }

    /// Reject the form: record [`FormResult::Rejected`] and close.
    pub fn reject(&mut self, cx: &mut Context<Self>) {
        self.result = Some(FormResult::Rejected);
        self.open = false;
        cx.notify();
        cx.emit(FormEvent::Closed(self.result));
    }

    /// Activate a tab page by widget name (the SetStyle test uses this).
    pub fn select_tab(
        &mut self,
        widget: &str,
        index: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.kind_of(widget) != Some(WidgetKind::TabWidget) {
            return false;
        }
        if !self.set_current_index(widget, index) {
            return false;
        }
        cx.notify();
        true
    }

    fn id_of(&self, widget: &str) -> Option<WidgetId> {
        self.form.widget_id_by_name(widget)
    }

    fn kind_of_id(&self, id: WidgetId) -> WidgetKind {
        WidgetKind::of(&self.form.widgets[id].class)
    }

    fn click_widget(&mut self, id: WidgetId, checkable: bool, cx: &mut Context<Self>) {
        self.last_action = Some(self.form.widgets[id].name.clone());
        if checkable {
            self.states[id].checked = !self.states[id].checked;
        }
        cx.notify();
    }

    fn toggle_checked(&mut self, id: WidgetId, cx: &mut Context<Self>) {
        self.last_action = Some(self.form.widgets[id].name.clone());
        self.states[id].checked = !self.states[id].checked;
        cx.notify();
    }

    fn select_radio(&mut self, id: WidgetId, cx: &mut Context<Self>) {
        self.last_action = Some(self.form.widgets[id].name.clone());
        self.states[id].checked = true;
        self.uncheck_siblings(id);
        cx.notify();
    }

    fn uncheck_siblings(&mut self, id: WidgetId) {
        let parent = self.form.widgets[id].parent;
        let name = self.form.widgets[id].name.clone();
        for sibling in 0..self.form.widgets.len() {
            if sibling == id {
                continue;
            }
            if self.form.widgets[sibling].parent == parent
                && self.form.widgets[sibling].name != name
                && self.kind_of_id(sibling) == WidgetKind::RadioButton
            {
                self.states[sibling].checked = false;
            }
        }
    }

    fn cycle_combo(&mut self, id: WidgetId, cx: &mut Context<Self>) {
        let items = self.form.widgets[id].combo_items.len();
        if items == 0 {
            return;
        }
        self.last_action = Some(self.form.widgets[id].name.clone());
        self.states[id].index = (self.states[id].index + 1) % items;
        cx.notify();
    }

    fn select_tab_id(&mut self, id: WidgetId, index: usize, cx: &mut Context<Self>) {
        if index < self.form.widgets[id].tab_pages.len() {
            self.states[id].index = index;
            cx.notify();
        }
    }

    /// The text a widget draws, including a spin/quantity suffix or unit.
    fn display_text(&self, id: WidgetId) -> String {
        let widget = &self.form.widgets[id];
        let state = &self.states[id];
        match self.kind_of_id(id) {
            WidgetKind::ComboBox => widget
                .combo_items
                .get(state.index)
                .map(|item| item.text.clone())
                .or_else(|| widget.current_text().map(str::to_string))
                .unwrap_or_else(|| state.text.clone()),
            WidgetKind::SpinBox => {
                format_number(state.number, decimals(widget, 0)) + &suffix(widget)
            }
            WidgetKind::DoubleSpinBox => {
                format_number(state.number, decimals(widget, 2)) + &suffix(widget)
            }
            WidgetKind::InputField | WidgetKind::QuantitySpinBox => {
                format_number(state.number, decimals(widget, 2)) + &unit(widget)
            }
            WidgetKind::GroupBox => widget.title().unwrap_or("").to_string(),
            WidgetKind::FileChooser => widget
                .string_property("fileName")
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| "Choose file…".to_string()),
            _ => state.text.clone(),
        }
    }

    fn render_child_container(
        &self,
        widget: &Widget,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(layout) = widget.layout {
            self.render_layout(layout, cx)
        } else {
            let mut column = div().flex().flex_col().gap_1();
            for child in &widget.children {
                column = column.child(self.render_widget(*child, cx));
            }
            column.into_any_element()
        }
    }

    fn render_widget(&self, id: WidgetId, cx: &mut Context<Self>) -> AnyElement {
        let widget = &self.form.widgets[id];
        let state = &self.states[id];
        if !state.visible {
            return div().into_any_element();
        }
        let selector = self.selectors[id];
        let enabled = state.enabled;
        let mut element = base(selector);
        if !enabled {
            element = element.opacity(0.45);
        }

        match self.kind_of_id(id) {
            WidgetKind::Label => element
                .text_sm()
                .text_color(rgb(FG))
                .child(self.display_text(id))
                .into_any_element(),
            WidgetKind::PushButton => {
                let checkable = matches!(
                    widget.property("checkable"),
                    Some(PropertyValue::Bool(true))
                );
                let checked = state.checked;
                let mut button = element
                    .id(selector)
                    .px_2()
                    .py_1()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(0xffffff26))
                    .bg(rgb(if checkable && checked { ACCENT } else { PANEL_ALT }))
                    .text_xs()
                    .text_color(rgb(FG))
                    .child(self.display_text(id));
                if enabled {
                    button = button.on_click(cx.listener(move |this, _event, _window, cx| {
                        this.click_widget(id, checkable, cx)
                    }));
                }
                button.into_any_element()
            }
            WidgetKind::CheckBox | WidgetKind::RadioButton => {
                let radio = self.kind_of_id(id) == WidgetKind::RadioButton;
                let marker = if radio {
                    if state.checked { "(o)" } else { "( )" }
                } else if state.checked {
                    "[x]"
                } else {
                    "[ ]"
                };
                let mut row = element
                    .id(selector)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(rgb(FG))
                    .child(marker)
                    .child(self.display_text(id));
                if enabled {
                    let handler = cx.listener(move |this, _event, _window, cx| {
                        if radio {
                            this.select_radio(id, cx)
                        } else {
                            this.toggle_checked(id, cx)
                        }
                    });
                    row = row.on_click(handler);
                }
                row.into_any_element()
            }
            WidgetKind::LineEdit => element
                .px_2()
                .py_1()
                .min_w(px(140.))
                .rounded_sm()
                .border_1()
                .border_color(rgb(0xffffff26))
                .bg(rgb(PANEL_ALT))
                .text_xs()
                .text_color(rgb(FG))
                .child(self.display_text(id))
                .into_any_element(),
            WidgetKind::SpinBox
            | WidgetKind::DoubleSpinBox
            | WidgetKind::InputField
            | WidgetKind::QuantitySpinBox => element
                .px_2()
                .py_1()
                .min_w(px(88.))
                .rounded_sm()
                .border_1()
                .border_color(rgb(0xffffff26))
                .bg(rgb(PANEL_ALT))
                .text_xs()
                .text_color(rgb(FG))
                .child(self.display_text(id))
                .into_any_element(),
            WidgetKind::ComboBox => {
                let mut combo = element
                    .id(selector)
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .min_w(px(120.))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(0xffffff26))
                    .bg(rgb(PANEL_ALT))
                    .text_xs()
                    .text_color(rgb(FG))
                    .child(self.display_text(id))
                    .child(div().text_color(rgb(MUTED)).child("▾"));
                if enabled {
                    combo = combo.on_click(cx.listener(move |this, _event, _window, cx| {
                        this.cycle_combo(id, cx)
                    }));
                }
                combo.into_any_element()
            }
            WidgetKind::GroupBox => {
                let mut group = element
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(0xffffff1a));
                if let Some(title) = widget.title().filter(|title| !title.is_empty()) {
                    group = group.child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(title.to_string()),
                    );
                }
                group
                    .child(self.render_child_container(widget, cx))
                    .into_any_element()
            }
            WidgetKind::Container => element
                .flex()
                .flex_col()
                .gap_1()
                .child(self.render_child_container(widget, cx))
                .into_any_element(),
            WidgetKind::Frame => element
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_sm()
                .border_1()
                .border_color(rgb(0xffffff1a))
                .child(self.render_child_container(widget, cx))
                .into_any_element(),
            WidgetKind::ScrollArea => element
                .id(selector)
                .flex()
                .flex_col()
                .gap_1()
                .max_h(px(220.))
                .overflow_y_scroll()
                .child(self.render_child_container(widget, cx))
                .into_any_element(),
            WidgetKind::TabWidget => {
                let mut tabs = div().flex().flex_row().gap_1();
                for (index, page) in widget.tab_pages.iter().enumerate() {
                    let tab_selector = self.tab_selectors[id][index];
                    let active = index == state.index;
                    let mut tab = div()
                        .id(tab_selector)
                        .debug_selector(move || tab_selector.to_string())
                        .px_2()
                        .py_1()
                        .rounded_sm()
                        .border_1()
                        .border_color(rgb(0xffffff1a))
                        .bg(rgb(if active { ACCENT } else { PANEL_ALT }))
                        .text_xs()
                        .text_color(rgb(if active { FG } else { MUTED }))
                        .child(page.title.clone());
                    if enabled {
                        tab = tab.on_click(cx.listener(move |this, _event, _window, cx| {
                            this.select_tab_id(id, index, cx)
                        }));
                    }
                    tabs = tabs.child(tab);
                }
                let content = widget
                    .tab_pages
                    .get(state.index)
                    .map(|page| self.render_widget(page.widget, cx))
                    .unwrap_or_else(|| div().into_any_element());
                element
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(tabs)
                    .child(content)
                    .into_any_element()
            }
            WidgetKind::DialogButtonBox => element
                .px_2()
                .py_1()
                .rounded_sm()
                .border_1()
                .border_color(rgb(0xffffff1a))
                .text_xs()
                .text_color(rgb(MUTED))
                .child("Dialog buttons")
                .into_any_element(),
            WidgetKind::ColorButton => {
                let swatch = color_of(widget).unwrap_or(0x6b7280);
                element
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .min_w(px(120.))
                    .rounded_sm()
                    .border_1()
                    .border_color(rgb(0xffffff26))
                    .bg(rgb(PANEL_ALT))
                    .child(
                        div()
                            .w(px(18.))
                            .h(px(12.))
                            .rounded_sm()
                            .bg(rgb(swatch))
                            .border_1()
                            .border_color(rgb(0xffffff40)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(MUTED))
                            .child(self.display_text(id)),
                    )
                    .into_any_element()
            }
            WidgetKind::FileChooser => element
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_2()
                .py_1()
                .min_w(px(160.))
                .rounded_sm()
                .border_1()
                .border_color(rgb(0xffffff26))
                .bg(rgb(PANEL_ALT))
                .text_xs()
                .text_color(rgb(MUTED))
                .child(self.display_text(id))
                .into_any_element(),
            WidgetKind::Placeholder => element
                .px_1()
                .rounded_sm()
                .border_1()
                .border_color(rgb(0xffffff1a))
                .text_xs()
                .text_color(rgb(MUTED))
                .child(format!("[{} {}]", widget.class, widget.name))
                .into_any_element(),
        }
    }

    fn render_layout(&self, layout_id: LayoutId, cx: &mut Context<Self>) -> AnyElement {
        let layout = &self.form.layouts[layout_id];
        match layout.class.as_str() {
            "QGridLayout" | "QFormLayout" => {
                let (columns, rows) = grid_extent(layout);
                let mut grid = div()
                    .grid()
                    .grid_cols(columns)
                    .gap_1()
                    .items_start()
                    .min_w_0();
                if rows > 1 {
                    grid = grid.grid_rows(rows);
                }
                for item in &layout.items {
                    let mut cell = div().min_w_0().child(self.render_item(item, cx));
                    if let Some(column) = item.column {
                        cell = cell
                            .col_start(column as i16 + 1)
                            .col_span(item.column_span.max(1) as u16);
                    }
                    if let Some(row) = item.row {
                        cell = cell.row_start(row as i16 + 1).row_span(item.row_span.max(1) as u16);
                    }
                    grid = grid.child(cell);
                }
                grid.into_any_element()
            }
            "QHBoxLayout" => {
                let mut row = div().flex().flex_row().items_center().gap_2().min_w_0();
                for item in &layout.items {
                    row = row.child(self.render_item(item, cx));
                }
                row.into_any_element()
            }
            _ => {
                let mut column = div().flex().flex_col().gap_1().min_w_0();
                for item in &layout.items {
                    column = column.child(self.render_item(item, cx));
                }
                column.into_any_element()
            }
        }
    }

    fn render_item(&self, item: &LayoutItem, cx: &mut Context<Self>) -> AnyElement {
        match item.kind {
            LayoutItemKind::Widget(id) => self.render_widget(id, cx),
            LayoutItemKind::Layout(id) => self.render_layout(id, cx),
            LayoutItemKind::Spacer(id) => {
                let spacer = &self.form.spacers[id];
                let size = spacer.size_hint.unwrap_or(ferrocad_ui::Size {
                    width: 0,
                    height: 0,
                });
                let expanding = spacer
                    .size_type
                    .as_deref()
                    .is_some_and(|kind| kind.contains("Expanding"));
                if expanding {
                    div()
                        .flex_1()
                        .min_w(px(size.width.max(0) as f32))
                        .min_h(px(size.height.max(0) as f32))
                        .into_any_element()
                } else {
                    div()
                        .w(px(size.width.max(0) as f32))
                        .h(px(size.height.max(0) as f32))
                        .into_any_element()
                }
            }
        }
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        let reject = self.reject_selector;
        let accept = self.accept_selector;
        div()
            .flex()
            .flex_row()
            .justify_end()
            .gap_2()
            .child(
                div()
                    .id(reject)
                    .debug_selector(move || reject.to_string())
                    .px_3()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(PANEL_ALT))
                    .text_xs()
                    .text_color(rgb(FG))
                    .child(self.reject_label.clone())
                    .on_click(cx.listener(|this, _event, _window, cx| this.reject(cx))),
            )
            .child(
                div()
                    .id(accept)
                    .debug_selector(move || accept.to_string())
                    .px_3()
                    .py_1()
                    .rounded_sm()
                    .bg(rgb(ACCENT))
                    .text_xs()
                    .text_color(rgb(FG))
                    .child(self.accept_label.clone())
                    .on_click(cx.listener(|this, _event, _window, cx| this.accept(cx))),
            )
            .into_any_element()
    }
}

impl Render for FormDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().size_full().into_any_element();
        }
        let root = self.form.root;
        let body = self.render_widget(root, cx);
        let footer = self.render_footer(cx);
        let panel = div()
            .id(self.panel_selector)
            .debug_selector({
                let selector = self.panel_selector;
                move || selector.to_string()
            })
            .w(px(560.))
            .max_h(px(780.))
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .bg(rgb(PANEL))
            .border_1()
            .border_color(rgb(0xffffff26))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(FG))
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .id(self.body_selector)
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(div().border_t_1().border_color(rgb(0xffffff14)).pt_2().child(footer));

        let backdrop = self.backdrop_selector;
        div()
            .id(backdrop)
            .debug_selector(move || backdrop.to_string())
            .absolute()
            .left(px(0.))
            .top(px(0.))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(rgba(0x0f1218cc))
            .occlude()
            .child(panel)
            .into_any_element()
    }
}

impl WidgetState {
    fn seed(widget: &Widget) -> Self {
        let text = widget
            .text()
            .or_else(|| widget.title())
            .or_else(|| widget.current_text())
            .unwrap_or("")
            .to_string();
        let number = match widget.property("quantity").or_else(|| widget.property("value")) {
            Some(PropertyValue::Int(value)) => *value as f64,
            Some(PropertyValue::Double(value)) => *value,
            _ => 0.0,
        };
        let checked = matches!(
            widget.property("checked"),
            Some(PropertyValue::Bool(true))
        );
        let index = match widget.property("currentIndex") {
            Some(PropertyValue::Int(index)) if *index >= 0 => *index as usize,
            _ => widget
                .current_text()
                .and_then(|text| widget.combo_items.iter().position(|item| item.text == text))
                .unwrap_or(0),
        };
        let enabled = !matches!(
            widget.property("enabled"),
            Some(PropertyValue::Bool(false))
        );
        let visible = !matches!(
            widget.property("visible"),
            Some(PropertyValue::Bool(false))
        );
        Self {
            text,
            number,
            checked,
            index,
            enabled,
            visible,
        }
    }
}

/// A `Div` carrying the widget's stable debug selector.
fn base(selector: &'static str) -> Div {
    div().debug_selector(move || selector.to_string())
}

fn grid_extent(layout: &ferrocad_ui::Layout) -> (u16, u16) {
    let mut columns = 1u16;
    let mut rows = 1u16;
    for item in &layout.items {
        if let Some(column) = item.column {
            columns = columns.max((column + item.column_span.max(1)) as u16);
        }
        if let Some(row) = item.row {
            rows = rows.max((row + item.row_span.max(1)) as u16);
        }
    }
    (columns.max(1), rows.max(1))
}

fn decimals(widget: &Widget, default: usize) -> usize {
    match widget.property("decimals") {
        Some(PropertyValue::Int(value)) if *value >= 0 => *value as usize,
        _ => default,
    }
}

fn suffix(widget: &Widget) -> String {
    widget
        .string_property("suffix")
        .filter(|suffix| !suffix.is_empty())
        .map(str::to_string)
        .unwrap_or_default()
}

fn unit(widget: &Widget) -> String {
    widget
        .string_property("unit")
        .filter(|unit| !unit.is_empty())
        .map(|unit| format!(" {unit}"))
        .unwrap_or_default()
}

fn color_of(widget: &Widget) -> Option<u32> {
    match widget.property("color") {
        Some(PropertyValue::Color(color)) => Some(
            ((color.red as u32) << 16) | ((color.green as u32) << 8) | color.blue as u32,
        ),
        _ => None,
    }
}

fn format_number(value: f64, decimals: usize) -> String {
    if decimals == 0 {
        if value.fract().abs() < f64::EPSILON {
            format!("{}", value as i64)
        } else {
            trim_fraction(format!("{value:.2}"))
        }
    } else {
        trim_fraction(format!("{value:.decimals$}"))
    }
}

fn trim_fraction(mut text: String) -> String {
    if text.contains('.') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    text
}

/// Intern a selector so `VisualTestContext::debug_bounds(&'static str)` can use it.
///
/// Selectors are unique per form and widget, so a process-wide set keeps memory
/// bounded across repeated dialog openings.
fn intern(text: String) -> &'static str {
    static SELECTORS: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut selectors = SELECTORS
        .get_or_init(|| Mutex::new(HashSet::new()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some(existing) = selectors.get(text.as_str()) {
        return existing;
    }
    let leaked: &'static str = Box::leak(text.into_boxed_str());
    selectors.insert(leaked);
    leaked
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};

    use super::*;

    /// Reuse Q1's upstream-lookup convention: `FERROCAD_UPSTREAM`, then the
    /// sibling checkout. Tests skip (and pass) when it is absent.
    fn upstream_ui_dir() -> Option<PathBuf> {
        if let Some(root) = std::env::var_os("FERROCAD_UPSTREAM") {
            let root = PathBuf::from(root);
            for candidate in [
                root.join("src/Mod/Draft/Resources/ui"),
                root.join("Mod/Draft/Resources/ui"),
                root.join("Resources/ui"),
                root.clone(),
            ] {
                if candidate.is_dir() {
                    return Some(candidate);
                }
            }
        }
        let local = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../freecad-upstream/src/Mod/Draft/Resources/ui");
        local.is_dir().then_some(local)
    }

    fn load(file: &str, name: &'static str) -> Option<FormDialog> {
        let directory = upstream_ui_dir()?;
        let path = directory.join(file);
        let form = Form::from_path(&path)
            .unwrap_or_else(|error| panic!("cannot parse {}: {error}", path.display()));
        Some(FormDialog::new(form, name))
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            let _ = window.draw(cx);
        });
    }

    fn click_at(cx: &mut VisualTestContext, selector: &'static str) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} has no bounds"));
        cx.simulate_click(bounds.center(), Modifiers::default());
    }

    fn open(
        cx: &mut TestAppContext,
        dialog: FormDialog,
    ) -> (VisualTestContext, Entity<FormDialog>) {
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), move |_window, cx| {
                cx.new(|_| dialog)
            })
            .unwrap()
        });
        let mut visual = VisualTestContext::from_window(window.into(), cx);
        let dialog: Entity<FormDialog> = window.root(&mut visual).unwrap();
        draw(&mut visual);
        (visual, dialog)
    }

    /// The macro skips a test body when the upstream checkout is absent.
    macro_rules! skip_without_upstream {
        () => {
            if upstream_ui_dir().is_none() {
                eprintln!("skipping: freecad-upstream is absent");
                return;
            }
        };
    }

    #[gpui::test]
    fn orthoarray_renders_its_initial_widgets(cx: &mut TestAppContext) {
        skip_without_upstream!();
        let dialog = load("TaskPanel_OrthoArray.ui", "OrthoArray").expect("form");
        let (mut cx, dialog) = open(cx, dialog);

        for selector in [
            "form-OrthoArray",
            "form-OrthoArray-button_linear_mode",
            "form-OrthoArray-radiobutton_x_axis",
            "form-OrthoArray-group_copies",
            "form-OrthoArray-spinbox_n_X",
            "form-OrthoArray-input_X_x",
            "form-OrthoArray-accept",
            "form-OrthoArray-reject",
        ] {
            assert!(
                cx.debug_bounds(selector).is_some(),
                "{selector} is rendered"
            );
        }

        dialog.read_with(&cx, |dialog, _| {
            assert_eq!(
                dialog.text_of("button_linear_mode").as_deref(),
                Some("Switch to Linear Mode")
            );
            assert_eq!(dialog.value_of("spinbox_n_X"), Some(2.0));
            assert_eq!(dialog.value_of("spinbox_n_Z"), Some(1.0));
            assert_eq!(dialog.checked_of("radiobutton_x_axis"), Some(true));
            assert_eq!(dialog.checked_of("radiobutton_y_axis"), Some(false));
            assert_eq!(dialog.kind_of("input_X_x"), Some(WidgetKind::InputField));
            assert_eq!(dialog.title(), "Orthogonal Array");
        });
    }

    #[gpui::test]
    fn a_radio_click_is_exclusive_and_accept_and_reject_set_the_result(
        cx: &mut TestAppContext,
    ) {
        skip_without_upstream!();
        let dialog = load("TaskPanel_OrthoArray.ui", "OrthoArray").expect("form");
        let (mut cx, dialog) = open(cx, dialog);

        click_at(&mut cx, "form-OrthoArray-radiobutton_y_axis");
        dialog.read_with(&cx, |dialog, _| {
            assert_eq!(dialog.checked_of("radiobutton_y_axis"), Some(true));
            assert_eq!(dialog.checked_of("radiobutton_x_axis"), Some(false));
            assert_eq!(dialog.last_action(), Some("radiobutton_y_axis"));
        });

        click_at(&mut cx, "form-OrthoArray-accept");
        dialog.read_with(&cx, |dialog, _| {
            assert_eq!(dialog.result(), Some(FormResult::Accepted));
            assert!(!dialog.is_open());
        });

        dialog.update(&mut cx, |dialog, cx| dialog.show(cx));
        draw(&mut cx);
        click_at(&mut cx, "form-OrthoArray-reject");
        dialog.read_with(&cx, |dialog, _| {
            assert_eq!(dialog.result(), Some(FormResult::Rejected));
            assert!(!dialog.is_open());
        });
    }

    #[gpui::test]
    fn polararray_renders_values_and_checkboxes(cx: &mut TestAppContext) {
        skip_without_upstream!();
        let dialog = load("TaskPanel_PolarArray.ui", "PolarArray").expect("form");
        let (mut cx, dialog) = open(cx, dialog);

        for selector in [
            "form-PolarArray",
            "form-PolarArray-spinbox_angle",
            "form-PolarArray-spinbox_number",
            "form-PolarArray-group_center",
            "form-PolarArray-input_c_x",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector} is rendered");
        }
        dialog.read_with(&cx, |dialog, _| {
            assert_eq!(dialog.value_of("spinbox_angle"), Some(360.0));
            assert_eq!(dialog.value_of("spinbox_number"), Some(5.0));
            assert_eq!(dialog.checked_of("checkbox_link"), Some(true));
            assert_eq!(dialog.checked_of("checkbox_fuse"), Some(false));
            assert_eq!(dialog.text_of("group_center").as_deref(), Some("Center of Rotation"));
        });
    }

    #[gpui::test]
    fn circulararray_renders_its_distances(cx: &mut TestAppContext) {
        skip_without_upstream!();
        let dialog = load("TaskPanel_CircularArray.ui", "CircularArray").expect("form");
        let (mut cx, dialog) = open(cx, dialog);

        for selector in [
            "form-CircularArray",
            "form-CircularArray-spinbox_r_distance",
            "form-CircularArray-spinbox_tan_distance",
            "form-CircularArray-spinbox_number",
            "form-CircularArray-spinbox_symmetry",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector} is rendered");
        }
        dialog.read_with(&cx, |dialog, _| {
            assert_eq!(dialog.value_of("spinbox_r_distance"), Some(200.0));
            assert_eq!(dialog.value_of("spinbox_tan_distance"), Some(100.0));
            assert_eq!(dialog.value_of("spinbox_number"), Some(3.0));
            assert_eq!(dialog.value_of("spinbox_symmetry"), Some(1.0));
        });
    }

    #[gpui::test]
    fn shapestring_renders_text_numeric_and_file_widgets(cx: &mut TestAppContext) {
        skip_without_upstream!();
        let dialog = load("TaskShapeString.ui", "ShapeString").expect("form");
        let (mut cx, dialog) = open(cx, dialog);

        for selector in [
            "form-ShapeString",
            "form-ShapeString-labelText",
            "form-ShapeString-leText",
            "form-ShapeString-sbHeight",
            "form-ShapeString-fcFontFile",
            "form-ShapeString-cbGlobalMode",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector} is rendered");
        }
        dialog.read_with(&cx, |dialog, _| {
            assert_eq!(dialog.value_of("sbHeight"), Some(10.0));
            assert_eq!(dialog.text_of("labelText").as_deref(), Some("Text"));
            assert_eq!(dialog.kind_of("leText"), Some(WidgetKind::LineEdit));
            assert_eq!(dialog.kind_of("fcFontFile"), Some(WidgetKind::FileChooser));
            assert_eq!(dialog.checked_of("cbGlobalMode"), Some(false));
        });
    }

    #[gpui::test]
    fn selectplane_renders_buttons_and_fields(cx: &mut TestAppContext) {
        skip_without_upstream!();
        let dialog = load("TaskSelectPlane.ui", "SelectPlane").expect("form");
        let (mut cx, dialog) = open(cx, dialog);

        for selector in [
            "form-SelectPlane",
            "form-SelectPlane-buttonTop",
            "form-SelectPlane-buttonFront",
            "form-SelectPlane-fieldOffset",
            "form-SelectPlane-fieldGridMainLine",
            "form-SelectPlane-buttonColor",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector} is rendered");
        }
        dialog.read_with(&cx, |dialog, _| {
            assert_eq!(dialog.text_of("buttonTop").as_deref(), Some("Top (XY)"));
            assert_eq!(
                dialog.text_of("buttonAlign").as_deref(),
                Some("Align to View")
            );
            assert_eq!(dialog.kind_of("buttonColor"), Some(WidgetKind::ColorButton));
            assert_eq!(
                dialog.kind_of("fieldGridMainLine"),
                Some(WidgetKind::SpinBox)
            );
        });
    }

    #[gpui::test]
    fn setstyle_renders_tabs_combos_and_color_buttons(cx: &mut TestAppContext) {
        skip_without_upstream!();
        let dialog = load("TaskPanel_SetStyle.ui", "SetStyle").expect("form");
        let (mut cx, dialog) = open(cx, dialog);

        for selector in [
            "form-SetStyle",
            "form-SetStyle-tabs",
            "form-SetStyle-tabs-tab-0",
            "form-SetStyle-comboPresets",
            "form-SetStyle-tab_Shape",
            "form-SetStyle-groupBox_1",
            "form-SetStyle-ShapeColor",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector} is rendered");
        }
        // The second page is only built once its tab is active.
        assert!(cx.debug_bounds("form-SetStyle-tab_Annotation").is_none());
        dialog.update(&mut cx, |dialog, cx| {
            assert!(dialog.select_tab("tabs", 1, cx));
        });
        draw(&mut cx);
        assert!(cx.debug_bounds("form-SetStyle-tab_Annotation").is_some());
        assert!(cx.debug_bounds("form-SetStyle-groupBox_3").is_some());

        dialog.read_with(&cx, |dialog, _| {
            assert_eq!(dialog.text_of("comboPresets").as_deref(), Some("Load preset"));
            assert_eq!(
                dialog.text_of("groupBox_1").as_deref(),
                Some("Shape Appearance")
            );
            assert_eq!(dialog.kind_of("ShapeColor"), Some(WidgetKind::ColorButton));
            assert_eq!(dialog.kind_of("TextFont"), Some(WidgetKind::ComboBox));
            assert_eq!(dialog.current_index_of("tabs"), Some(1));
        });
    }

    #[gpui::test]
    fn an_unsupported_widget_is_a_labelled_placeholder(cx: &mut TestAppContext) {
        const SOURCE: &str = r#"
<ui version="4.0">
 <class>OddForm</class>
 <widget class="QWidget" name="Form">
  <layout class="QVBoxLayout" name="column">
   <item><widget class="QTreeView" name="tree"/></item>
   <item><widget class="QLabel" name="caption">
    <property name="text"><string>caption</string></property>
   </widget></item>
  </layout>
 </widget>
</ui>
"#;
        let form = Form::parse_str(SOURCE).expect("inline form");
        let dialog = FormDialog::new(form, "Odd");
        let (mut cx, dialog) = open(cx, dialog);

        assert!(cx.debug_bounds("form-Odd-tree").is_some());
        assert!(cx.debug_bounds("form-Odd-caption").is_some());
        dialog.read_with(&cx, |dialog, _| {
            assert_eq!(dialog.kind_of("tree"), Some(WidgetKind::Placeholder));
            assert!(!WidgetKind::Placeholder.is_interactive());
            assert_eq!(dialog.text_of("caption").as_deref(), Some("caption"));
        });
    }

    #[gpui::test]
    fn accept_reject_and_close_drive_the_lifecycle(cx: &mut TestAppContext) {
        const SOURCE: &str = r#"
<ui version="4.0">
 <class>SmallForm</class>
 <widget class="QWidget" name="Form">
  <layout class="QVBoxLayout" name="column">
   <item><widget class="QCheckBox" name="flag">
    <property name="text"><string>Flag</string></property>
    <property name="checked"><bool>true</bool></property>
   </widget></item>
  </layout>
 </widget>
</ui>
"#;
        let form = Form::parse_str(SOURCE).expect("inline form");
        let dialog = cx.update(|cx| cx.new(|_| FormDialog::new(form, "Small")));

        assert!(cx.update(|cx| dialog.read(cx).is_open()));
        cx.update(|cx| dialog.update(cx, |dialog, cx| dialog.accept(cx)));
        cx.update(|cx| {
            let dialog = dialog.read(cx);
            assert_eq!(dialog.result(), Some(FormResult::Accepted));
            assert_eq!(dialog.result().map(FormResult::code), Some(1));
            assert!(!dialog.is_open());
        });

        cx.update(|cx| dialog.update(cx, |dialog, cx| dialog.show(cx)));
        cx.update(|cx| {
            let dialog = dialog.read(cx);
            assert!(dialog.is_open());
            assert_eq!(dialog.result(), None);
            assert_eq!(dialog.checked_of("flag"), Some(true));
        });
        cx.update(|cx| dialog.update(cx, |dialog, cx| dialog.reject(cx)));
        cx.update(|cx| {
            let dialog = dialog.read(cx);
            assert_eq!(dialog.result(), Some(FormResult::Rejected));
            assert_eq!(dialog.result().map(FormResult::code), Some(0));
        });

        cx.update(|cx| dialog.update(cx, |dialog, cx| dialog.show(cx)));
        let taken = cx.update(|cx| {
            dialog.update(cx, |dialog, cx| {
                dialog.close(cx);
                dialog.take_result()
            })
        });
        assert_eq!(taken, None, "close records no result");
        cx.update(|cx| assert!(!dialog.read(cx).is_open()));
    }
}
