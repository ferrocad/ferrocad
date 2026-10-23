//! In-memory fixture coverage and error behaviour.
//!
//! The fixture exercises every construct Q1 claims to model: nested layouts with
//! grid spans and stretch, typed properties, a custom widget, a spacer, combo
//! items (including an icon), tab pages, resources, connections and tab order.

use ferrocad_ui::{
    Color, Form, LayoutItemKind, Point, PropertyValue, Rect, Size, SpacerOrientation, UiError,
    UiErrorKind, Widget,
};

const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<ui version="4.0">
 <class>FixtureDialog</class>
 <widget class="QDialog" name="Dialog">
  <property name="geometry">
   <rect><x>10</x><y>20</y><width>300</width><height>200</height></rect>
  </property>
  <property name="windowTitle"><string>Fixture</string></property>
  <property name="modal"><bool>true</bool></property>
  <property name="windowModality"><enum>Qt::ApplicationModal</enum></property>
  <property name="magic"><number>42</number></property>
  <property name="ratio"><double>0.5</double></property>
  <property name="tags">
   <stringlist><string>a</string><string>b</string></stringlist>
  </property>
  <property name="origin"><point><x>1</x><y>-2</y></point></property>
  <property name="sizeHint"><size><width>10</width><height>20</height></size></property>
  <property name="paletteColor">
   <color><red>1</red><green>2</green><blue>3</blue><alpha>4</alpha></color>
  </property>
  <property name="flags">
   <set>Qt::AlignRight|Qt::AlignVCenter</set>
  </property>
  <property name="policy">
   <sizepolicy hsizetype="Minimum" vsizetype="Fixed">
    <horstretch>0</horstretch><verstretch>2</verstretch>
   </sizepolicy>
  </property>
  <property name="icon">
   <iconset theme="gtk-save"><normaloff>save.png</normaloff></iconset>
  </property>
  <layout class="QVBoxLayout" name="main" columnstretch="1,0" rowstretch="2">
   <property name="spacing"><number>6</number></property>
   <item row="0" column="0">
    <widget class="QGroupBox" name="group">
     <property name="title"><string>Group</string></property>
     <layout class="QGridLayout" name="grid">
      <item row="0" column="0" colspan="2">
       <widget class="Gui::FixtureWidget" name="custom">
        <property name="text"><cstring>Custom</cstring></property>
        <property name="rawValue" stdset="0"><string>1 mm</string></property>
       </widget>
      </item>
      <item row="1" column="0">
       <spacer name="spacer">
        <property name="orientation"><enum>Qt::Vertical</enum></property>
        <property name="sizeHint"><size><width>20</width><height>40</height></size></property>
        <property name="sizeType"><enum>QSizePolicy::Expanding</enum></property>
       </spacer>
      </item>
      <item row="1" column="1">
       <layout class="QHBoxLayout" name="inner">
        <item>
         <widget class="QComboBox" name="combo">
          <item><property name="text"><string>One</string></property></item>
          <item>
           <property name="text"><string>Two</string></property>
           <property name="icon">
            <iconset theme="gtk-open"><normaloff>open.png</normaloff></iconset>
           </property>
          </item>
          <property name="currentIndex"><number>1</number></property>
         </widget>
        </item>
       </layout>
      </item>
     </layout>
    </widget>
   </item>
   <item row="1" column="0">
    <widget class="QTabWidget" name="tabs">
     <widget class="QWidget" name="page1">
      <attribute name="title"><string>First</string></attribute>
      <layout class="QVBoxLayout" name="page1Layout">
       <item>
        <widget class="QLabel" name="label">
         <property name="text"><string>Hi</string></property>
        </widget>
       </item>
      </layout>
     </widget>
     <widget class="QWidget" name="page2">
      <attribute name="title"><string>Second</string></attribute>
     </widget>
    </widget>
   </item>
  </layout>
 </widget>
 <customwidgets>
  <customwidget>
   <class>Gui::FixtureWidget</class>
   <extends>QWidget</extends>
   <header>Fixture.h</header>
  </customwidget>
 </customwidgets>
 <resources>
  <include location="../Fixture.qrc"/>
 </resources>
 <connections>
  <connection>
   <sender>combo</sender>
   <signal>currentIndexChanged(int)</signal>
   <receiver>Dialog</receiver>
   <slot>accept()</slot>
   <hints>
    <hint type="sourcelabel"><x>1</x><y>2</y></hint>
   </hints>
  </connection>
 </connections>
 <tabstops>
  <tabstop>combo</tabstop>
  <tabstop>label</tabstop>
 </tabstops>
 <layoutdefault spacing="6" margin="11"/>
</ui>
"#;

fn parse_fixture() -> Form {
    Form::parse_str(FIXTURE).unwrap_or_else(|error| panic!("fixture did not parse: {error}"))
}

fn widget<'a>(form: &'a Form, name: &str) -> &'a Widget {
    form.widget_by_name(name)
        .unwrap_or_else(|| panic!("no widget named {name}"))
}

fn widget_item(item: &ferrocad_ui::LayoutItem) -> usize {
    match item.kind {
        LayoutItemKind::Widget(widget) => widget,
        _ => panic!("expected a widget item"),
    }
}

#[test]
fn parses_the_nested_fixture_into_an_ordered_tree() {
    let form = parse_fixture();

    assert_eq!(form.ui_version, "4.0");
    assert_eq!(form.class, "FixtureDialog");
    assert_eq!(form.root, 0);
    assert_eq!(form.widgets.len(), 8);
    assert_eq!(form.layouts.len(), 4);
    assert_eq!(form.spacers.len(), 1);

    // Document order is pre-order and stable.
    let names: Vec<&str> = form
        .widgets
        .iter()
        .map(|widget| widget.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "Dialog", "group", "custom", "combo", "tabs", "page1", "label", "page2"
        ]
    );

    // Parent/child and layout relationships.
    assert_eq!(form.widgets[0].parent, None);
    assert_eq!(form.widgets[0].children, [1, 4]);
    assert_eq!(form.widgets[1].children, [2, 3]);
    assert_eq!(form.widgets[4].children, [5, 7]);
    assert_eq!(form.widgets[1].layout, Some(1));
    assert_eq!(form.widgets[2].owning_layout, Some(1));
    assert_eq!(form.widgets[3].owning_layout, Some(2));
    assert_eq!(form.widgets[5].owning_layout, None);
    assert_eq!(form.widgets[5].layout, Some(3));
    assert_eq!(form.layouts[0].parent, Some(0));
    assert_eq!(form.layouts[1].parent, Some(1));
    assert_eq!(form.layouts[2].parent, Some(1));
    assert_eq!(form.layouts[3].parent, Some(5));

    // Nested layout items carry position, span and stretch.
    let main = &form.layouts[0];
    assert_eq!(main.class, "QVBoxLayout");
    assert_eq!(main.name.as_deref(), Some("main"));
    assert_eq!(main.items.len(), 2);
    assert_eq!(main.row_stretch, [2]);
    assert_eq!(main.column_stretch, [1, 0]);
    assert!(main.row_minimum_height.is_empty());
    assert!(main.column_minimum_width.is_empty());
    assert_eq!(widget_item(&main.items[0]), 1);
    assert_eq!(widget_item(&main.items[1]), 4);
    assert_eq!(
        main.properties[0].value,
        PropertyValue::Int(6),
        "layout property parsed"
    );

    let grid = &form.layouts[1];
    assert_eq!(grid.class, "QGridLayout");
    assert_eq!(grid.items.len(), 3);
    assert_eq!(grid.items[0].row, Some(0));
    assert_eq!(grid.items[0].column, Some(0));
    assert_eq!(grid.items[0].row_span, 1);
    assert_eq!(grid.items[0].column_span, 2);
    assert_eq!(widget_item(&grid.items[0]), 2);
    match grid.items[1].kind {
        LayoutItemKind::Spacer(spacer) => assert_eq!(spacer, 0),
        _ => panic!("expected a spacer item"),
    }
    match grid.items[2].kind {
        LayoutItemKind::Layout(nested) => assert_eq!(nested, 2),
        _ => panic!("expected a nested layout item"),
    }

    let inner = &form.layouts[2];
    assert_eq!(inner.class, "QHBoxLayout");
    assert_eq!(inner.items.len(), 1);
    assert_eq!(widget_item(&inner.items[0]), 3);

    let page_layout = &form.layouts[3];
    assert_eq!(page_layout.class, "QVBoxLayout");
    assert_eq!(widget_item(&page_layout.items[0]), 6);
}

#[test]
fn properties_are_typed_and_keep_their_qt_names() {
    let form = parse_fixture();
    let root = form.root_widget();

    assert_eq!(
        root.property("geometry"),
        Some(&PropertyValue::Rect(Rect {
            x: 10,
            y: 20,
            width: 300,
            height: 200,
        }))
    );
    // The convenience field and the property agree.
    assert_eq!(root.geometry, Some(Rect { x: 10, y: 20, width: 300, height: 200 }));
    assert_eq!(
        root.property("windowTitle"),
        Some(&PropertyValue::String("Fixture".to_owned()))
    );
    assert_eq!(root.property("modal"), Some(&PropertyValue::Bool(true)));
    assert_eq!(
        root.property("windowModality"),
        Some(&PropertyValue::Enum("Qt::ApplicationModal".to_owned()))
    );
    assert_eq!(root.property("magic"), Some(&PropertyValue::Int(42)));
    assert_eq!(root.property("ratio"), Some(&PropertyValue::Double(0.5)));
    assert_eq!(
        root.property("tags"),
        Some(&PropertyValue::StringList(vec![
            "a".to_owned(),
            "b".to_owned()
        ]))
    );
    assert_eq!(
        root.property("origin"),
        Some(&PropertyValue::Point(Point { x: 1, y: -2 }))
    );
    assert_eq!(
        root.property("sizeHint"),
        Some(&PropertyValue::Size(Size {
            width: 10,
            height: 20,
        }))
    );
    assert_eq!(
        root.property("paletteColor"),
        Some(&PropertyValue::Color(Color {
            red: 1,
            green: 2,
            blue: 3,
            alpha: Some(4),
        }))
    );
    assert_eq!(
        root.property("flags"),
        Some(&PropertyValue::Set(vec![
            "Qt::AlignRight".to_owned(),
            "Qt::AlignVCenter".to_owned()
        ]))
    );
    match root.property("policy") {
        Some(PropertyValue::SizePolicy(policy)) => {
            assert_eq!(policy.horizontal, "Minimum");
            assert_eq!(policy.vertical, "Fixed");
            assert_eq!(policy.horizontal_stretch, 0);
            assert_eq!(policy.vertical_stretch, 2);
        }
        other => panic!("expected a sizepolicy, got {other:?}"),
    }
    match root.property("icon") {
        Some(PropertyValue::IconSet(icon)) => {
            assert_eq!(icon.theme.as_deref(), Some("gtk-save"));
            assert_eq!(icon.variants.len(), 1);
            assert_eq!(icon.variants[0].state, "normaloff");
            assert_eq!(icon.variants[0].file, "save.png");
        }
        other => panic!("expected an iconset, got {other:?}"),
    }

    // `stdset="0"` is preserved; the Qt default (absent) is true.
    let custom = widget(&form, "custom");
    assert_eq!(
        custom.property("text"),
        Some(&PropertyValue::CString("Custom".to_owned()))
    );
    let raw_value = custom
        .properties
        .iter()
        .find(|property| property.name == "rawValue")
        .expect("rawValue property");
    assert!(!raw_value.std_set);
    let text = custom
        .properties
        .iter()
        .find(|property| property.name == "text")
        .expect("text property");
    assert!(text.std_set);

    assert_eq!(widget(&form, "group").title(), Some("Group"));
    assert_eq!(widget(&form, "label").text(), Some("Hi"));
}

#[test]
fn parses_registry_spacers_combo_items_tabs_connections_and_tab_order() {
    let form = parse_fixture();

    assert_eq!(form.custom_widgets.len(), 1);
    let custom = form.custom_widget("Gui::FixtureWidget").expect("registry entry");
    assert_eq!(custom.class, "Gui::FixtureWidget");
    assert_eq!(custom.extends.as_deref(), Some("QWidget"));
    assert_eq!(custom.header.as_deref(), Some("Fixture.h"));

    let spacer = &form.spacers[0];
    assert_eq!(spacer.name, "spacer");
    assert_eq!(spacer.orientation, Some(SpacerOrientation::Vertical));
    assert_eq!(spacer.size_hint, Some(Size { width: 20, height: 40 }));
    assert_eq!(spacer.size_type.as_deref(), Some("QSizePolicy::Expanding"));

    let combo = widget(&form, "combo");
    assert_eq!(combo.combo_items.len(), 2);
    assert_eq!(combo.combo_items[0].text, "One");
    assert_eq!(combo.combo_items[1].text, "Two");
    assert_eq!(
        combo.combo_items[1]
            .icon
            .as_ref()
            .and_then(|icon| icon.theme.as_deref()),
        Some("gtk-open")
    );
    assert_eq!(
        combo.combo_items[1].icon.as_ref().unwrap().variants[0].file,
        "open.png"
    );
    assert_eq!(combo.property("currentIndex"), Some(&PropertyValue::Int(1)));

    let tabs = widget(&form, "tabs");
    assert_eq!(tabs.tab_pages.len(), 2);
    assert_eq!(tabs.tab_pages[0].title, "First");
    assert_eq!(tabs.tab_pages[0].widget, 5);
    assert_eq!(tabs.tab_pages[1].title, "Second");
    assert_eq!(tabs.tab_pages[1].widget, 7);

    assert_eq!(form.resources.len(), 1);
    assert_eq!(form.resources[0].location, "../Fixture.qrc");

    assert_eq!(form.connections.len(), 1);
    let connection = &form.connections[0];
    assert_eq!(connection.sender, "combo");
    assert_eq!(connection.signal, "currentIndexChanged(int)");
    assert_eq!(connection.receiver, "Dialog");
    assert_eq!(connection.slot, "accept()");
    assert_eq!(connection.hints.len(), 1);
    assert_eq!(connection.hints[0].kind, "sourcelabel");
    assert_eq!(connection.hints[0].x, Some(1));
    assert_eq!(connection.hints[0].y, Some(2));

    assert_eq!(form.tab_order, ["combo", "label"]);
    assert_eq!(
        form.layout_default,
        Some(ferrocad_ui::LayoutDefault {
            margin: Some(11),
            spacing: Some(6),
        })
    );

    // Name lookup is deterministic and returns the first occurrence.
    assert_eq!(form.widget_id_by_name("combo"), Some(3));
    assert_eq!(form.widget_id_by_name("missing"), None);
}

fn expect_error(source: &str) -> UiError {
    match Form::parse_str(source) {
        Ok(_) => panic!("expected an error for: {source:?}"),
        Err(error) => {
            assert!(error.line >= 1, "line must be 1-based");
            assert!(error.column >= 1, "column must be 1-based");
            error
        }
    }
}

#[test]
fn structural_errors_name_the_expectation_and_position() {
    let missing_name = "<ui version=\"4.0\">\n <class>X</class>\n <widget class=\"QWidget\"/>\n</ui>";
    let error = expect_error(missing_name);
    assert_eq!(error.line, 3);
    assert_eq!(error.column, 2);
    assert_eq!(error.offset, Some(missing_name.find("<widget").unwrap()));
    assert!(matches!(
        &error.kind,
        UiErrorKind::MissingAttribute { element, attribute }
            if element == "widget" && attribute == "name"
    ));

    let unknown_class = "<ui version=\"4.0\">\n <class>X</class>\n \
                         <widget class=\"QDefinitelyNotAWidget\" name=\"x\"/>\n</ui>";
    let error = expect_error(unknown_class);
    assert_eq!(error.line, 3);
    assert!(matches!(
        &error.kind,
        UiErrorKind::UnknownClass { class, .. } if class == "QDefinitelyNotAWidget"
    ));

    let unexpected = "<ui version=\"4.0\"><class>X</class>\
                      <widget class=\"QWidget\" name=\"x\"><bogus/></widget></ui>";
    let error = expect_error(unexpected);
    assert!(matches!(
        &error.kind,
        UiErrorKind::UnexpectedElement { found, .. } if found == "bogus"
    ));

    let unsupported_value = "<ui version=\"4.0\"><class>X</class>\
                             <widget class=\"QWidget\" name=\"x\">\
                             <property name=\"font\"><font/></property></widget></ui>";
    let error = expect_error(unsupported_value);
    assert!(matches!(
        &error.kind,
        UiErrorKind::UnsupportedValue { element } if element == "font"
    ));

    let empty_item = "<ui version=\"4.0\"><class>X</class>\
                      <widget class=\"QWidget\" name=\"x\">\
                      <layout class=\"QVBoxLayout\" name=\"l\"><item/></layout>\
                      </widget></ui>";
    let error = expect_error(empty_item);
    assert!(matches!(&error.kind, UiErrorKind::MissingElement { .. }));

    let unknown_layout = "<ui version=\"4.0\"><class>X</class>\
                          <widget class=\"QWidget\" name=\"x\">\
                          <layout class=\"QTotallyFakeLayout\" name=\"l\"/></widget></ui>";
    let error = expect_error(unknown_layout);
    assert!(matches!(
        &error.kind,
        UiErrorKind::UnknownClass { class, .. } if class == "QTotallyFakeLayout"
    ));

    let duplicate_root = "<ui version=\"4.0\"><class>X</class>\
                          <widget class=\"QWidget\" name=\"a\"/>\
                          <widget class=\"QWidget\" name=\"b\"/></ui>";
    let error = expect_error(duplicate_root);
    assert!(matches!(&error.kind, UiErrorKind::Duplicate { .. }));

    let no_root = "<ui version=\"4.0\"><class>X</class></ui>";
    let error = expect_error(no_root);
    assert!(matches!(&error.kind, UiErrorKind::MissingElement { .. }));

    let bad_bool = "<ui version=\"4.0\"><class>X</class>\
                    <widget class=\"QWidget\" name=\"x\">\
                    <property name=\"visible\"><bool>yes</bool></property></widget></ui>";
    let error = expect_error(bad_bool);
    assert!(matches!(&error.kind, UiErrorKind::InvalidValue { .. }));

    let bad_geometry = "<ui version=\"4.0\"><class>X</class>\
                        <widget class=\"QWidget\" name=\"x\">\
                        <property name=\"geometry\"><string>oops</string></property></widget></ui>";
    let error = expect_error(bad_geometry);
    assert!(matches!(&error.kind, UiErrorKind::InvalidValue { .. }));

    let not_ui = "<form><widget class=\"QWidget\" name=\"x\"/></form>";
    let error = expect_error(not_ui);
    assert!(matches!(
        &error.kind,
        UiErrorKind::UnexpectedElement { parent, .. } if parent == "document"
    ));
}

#[test]
fn malformed_xml_is_an_error_not_a_panic() {
    let source = "<ui version=\"4.0\">\n <class>X</class>\n \
                  <widget class=\"QWidget\" name=\"x\">\n</ui>";
    let error = expect_error(source);
    assert!(matches!(&error.kind, UiErrorKind::Xml { .. }));
    assert!(error.line >= 2, "XML syntax errors carry a source position");

    let error = expect_error("");
    assert!(matches!(&error.kind, UiErrorKind::Xml { .. }));
}

#[test]
fn nesting_past_the_limit_is_an_error() {
    let mut source = String::from("<ui version=\"4.0\"><class>X</class>");
    for index in 0..300 {
        source.push_str(&format!(
            "<widget class=\"QWidget\" name=\"w{index}\">\
             <layout class=\"QVBoxLayout\" name=\"l{index}\"><item>"
        ));
    }
    source.push_str("<widget class=\"QWidget\" name=\"leaf\"/>");
    for _ in 0..300 {
        source.push_str("</item></layout></widget>");
    }
    source.push_str("</ui>");

    let error = expect_error(&source);
    assert!(matches!(&error.kind, UiErrorKind::NestingTooDeep { .. }));
}

#[test]
fn missing_file_is_an_io_error_at_one_one() {
    let error = Form::from_path("/definitely/not/a/form.ui").expect_err("missing file");
    assert!(matches!(&error.kind, UiErrorKind::Io { .. }));
    assert_eq!((error.line, error.column), (1, 1));
    assert_eq!(error.offset, None);
}
