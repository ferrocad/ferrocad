//! Q1 acceptance: parse all 19 upstream Draft `.ui` files and reproduce the
//! counts pinned by `contracts/draft-forms.json` and `tools/draft_contract.py`.
//!
//! The test skips (and passes) when the upstream checkout is absent. Point
//! `FERROCAD_UPSTREAM` at the checkout root, at `src/Mod/Draft`, or at its
//! `Resources/ui` directory to run it from elsewhere.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use ferrocad_ui::{Form, LayoutItemKind};

/// `(file name, widget count, distinct class count)` for every upstream form.
/// The widget totals are the ones pinned in `contracts/draft-forms.json`.
const FORMS: &[(&str, usize, usize)] = &[
    ("TaskPanel_CircularArray.ui", 19, 8),
    ("TaskPanel_OrthoArray.ui", 39, 8),
    ("TaskPanel_PolarArray.ui", 15, 8),
    ("TaskPanel_SetStyle.ui", 72, 13),
    ("TaskSelectPlane.ui", 26, 7),
    ("TaskShapeString.ui", 15, 7),
    ("dialogHatch.ui", 11, 6),
    ("dialogLayers.ui", 9, 3),
    ("dialog_AnnotationStyleEditor.ui", 53, 15),
    ("preferences-draft.ui", 27, 8),
    ("preferences-draftinterface.ui", 50, 5),
    ("preferences-draftsnap.ui", 29, 8),
    ("preferences-drafttexts.ui", 44, 11),
    ("preferences-draftvisual.ui", 13, 6),
    ("preferences-dwg.ui", 7, 5),
    ("preferences-dxf-import.ui", 11, 7),
    ("preferences-dxf.ui", 35, 6),
    ("preferences-oca.ui", 3, 3),
    ("preferences-svg.ui", 17, 7),
];

/// The exact global class inventory from `qt-strategy.md` §2.3 (32 classes).
const CLASSES: &[(&str, usize)] = &[
    ("Gui::ColorButton", 11),
    ("Gui::FileChooser", 2),
    ("Gui::InputField", 24),
    ("Gui::PrefCheckBox", 42),
    ("Gui::PrefColorButton", 5),
    ("Gui::PrefComboBox", 10),
    ("Gui::PrefDoubleSpinBox", 7),
    ("Gui::PrefFileChooser", 2),
    ("Gui::PrefFontBox", 1),
    ("Gui::PrefLineEdit", 29),
    ("Gui::PrefQuantitySpinBox", 1),
    ("Gui::PrefRadioButton", 4),
    ("Gui::PrefSpinBox", 10),
    ("Gui::PrefUnitSpinBox", 7),
    ("Gui::QuantitySpinBox", 14),
    ("QCheckBox", 13),
    ("QComboBox", 9),
    ("QDialog", 3),
    ("QDialogButtonBox", 2),
    ("QDoubleSpinBox", 6),
    ("QFontComboBox", 2),
    ("QFrame", 1),
    ("QGroupBox", 42),
    ("QLabel", 170),
    ("QLineEdit", 3),
    ("QPushButton", 30),
    ("QRadioButton", 7),
    ("QScrollArea", 1),
    ("QSpinBox", 16),
    ("QTabWidget", 1),
    ("QTreeView", 1),
    ("QWidget", 19),
];

/// The six core panels, whose per-form distinct property count must stay in the
/// 12–16 range measured for Q1.
const CORE_FORMS: &[&str] = &[
    "TaskPanel_OrthoArray.ui",
    "TaskPanel_PolarArray.ui",
    "TaskPanel_CircularArray.ui",
    "TaskShapeString.ui",
    "TaskSelectPlane.ui",
    "TaskPanel_SetStyle.ui",
];

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

fn read_forms() -> Option<Vec<(String, Form)>> {
    let directory = upstream_ui_dir()?;
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()))
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "ui"))
        .collect();
    paths.sort();
    Some(
        paths
            .into_iter()
            .map(|path| {
                let name = path
                    .file_name()
                    .expect("file name")
                    .to_string_lossy()
                    .into_owned();
                let form = Form::from_path(&path)
                    .unwrap_or_else(|error| panic!("cannot parse {}: {error}", path.display()));
                (name, form)
            })
            .collect(),
    )
}

fn property_names(form: &Form) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for widget in &form.widgets {
        for property in &widget.properties {
            names.insert(property.name.clone());
        }
        for item in &widget.combo_items {
            for property in &item.properties {
                names.insert(property.name.clone());
            }
        }
    }
    for layout in &form.layouts {
        for property in &layout.properties {
            names.insert(property.name.clone());
        }
    }
    for spacer in &form.spacers {
        for property in &spacer.properties {
            names.insert(property.name.clone());
        }
    }
    names
}

/// Structural invariants that must hold for every parsed form, not just the
/// fixture: one reachable root, consistent parent/child links, and layout items
/// that point back at their owning layout.
fn assert_invariants(name: &str, form: &Form) {
    assert_eq!(form.root, 0, "{name}: root must be arena index 0");
    assert!(!form.widgets.is_empty(), "{name}: no widgets");

    let mut seen = vec![false; form.widgets.len()];
    let mut stack = vec![form.root];
    while let Some(id) = stack.pop() {
        assert!(id < form.widgets.len(), "{name}: child id {id} out of range");
        assert!(!seen[id], "{name}: widget {id} reached twice");
        seen[id] = true;
        for &child in &form.widgets[id].children {
            assert_eq!(
                form.widgets[child].parent,
                Some(id),
                "{name}: widget {child} is not parented by {id}"
            );
            stack.push(child);
        }
    }
    assert!(
        seen.iter().all(|&visited| visited),
        "{name}: unreachable widgets"
    );

    for (id, layout) in form.layouts.iter().enumerate() {
        for item in &layout.items {
            match item.kind {
                LayoutItemKind::Widget(widget) => assert_eq!(
                    form.widgets[widget].owning_layout,
                    Some(id),
                    "{name}: widget {widget} does not name layout {id}"
                ),
                LayoutItemKind::Layout(nested) => assert_eq!(
                    form.layouts[nested].parent, layout.parent,
                    "{name}: nested layout {nested} has the wrong owner widget"
                ),
                LayoutItemKind::Spacer(spacer) => {
                    assert!(spacer < form.spacers.len(), "{name}: bad spacer id")
                }
            }
        }
    }

    for (id, widget) in form.widgets.iter().enumerate() {
        if let Some(layout) = widget.layout {
            assert_eq!(
                form.layouts[layout].parent,
                Some(id),
                "{name}: widget {id} owns a layout owned by someone else"
            );
        }
        for page in &widget.tab_pages {
            assert!(
                page.widget < form.widgets.len(),
                "{name}: bad tab page id"
            );
        }
    }
}

#[test]
fn parses_every_upstream_draft_form_and_reproduces_the_pinned_counts() {
    let Some(forms) = read_forms() else {
        eprintln!(
            "skipping: upstream Draft .ui directory not found \
             (set FERROCAD_UPSTREAM to enable this test)"
        );
        return;
    };

    assert_eq!(forms.len(), 19, "upstream file count");

    let mut class_instances: BTreeMap<String, usize> = BTreeMap::new();
    let mut widget_total = 0usize;
    let mut layouts = 0usize;
    let mut spacers = 0usize;
    let mut custom_widgets = 0usize;
    let mut connections = 0usize;
    let mut tab_order = 0usize;
    let mut combo_items = 0usize;
    let mut tab_pages = 0usize;
    let mut resources = 0usize;

    for (name, form) in &forms {
        let expected = FORMS
            .iter()
            .find(|(file, _, _)| file == name)
            .unwrap_or_else(|| panic!("unexpected upstream form {name}"));

        assert_invariants(name, form);
        assert_eq!(
            form.widgets.len(),
            expected.1,
            "{name}: widget count changed"
        );
        assert_eq!(
            form.class_counts().len(),
            expected.2,
            "{name}: class count changed"
        );
        assert_eq!(form.ui_version, "4.0", "{name}: unexpected ui version");

        widget_total += form.widgets.len();
        layouts += form.layouts.len();
        spacers += form.spacers.len();
        custom_widgets += form.custom_widgets.len();
        connections += form.connections.len();
        tab_order += form.tab_order.len();
        resources += form.resources.len();
        combo_items += form
            .widgets
            .iter()
            .map(|widget| widget.combo_items.len())
            .sum::<usize>();
        tab_pages += form
            .widgets
            .iter()
            .map(|widget| widget.tab_pages.len())
            .sum::<usize>();

        for (class, count) in form.class_counts() {
            *class_instances.entry(class.to_owned()).or_insert(0usize) += count;
        }
    }

    assert_eq!(widget_total, 495, "total widget instances");
    assert_eq!(class_instances.len(), 32, "distinct widget classes");
    let expected_classes: BTreeMap<String, usize> = CLASSES
        .iter()
        .map(|(class, count)| ((*class).to_owned(), *count))
        .collect();
    assert_eq!(
        class_instances, expected_classes,
        "class inventory changed"
    );

    let (plain_classes, plain_instances) = class_instances
        .iter()
        .filter(|(class, _)| !class.starts_with("Gui::"))
        .fold((0usize, 0usize), |(classes, instances), (_, count)| {
            (classes + 1, instances + *count)
        });
    let (custom_classes, custom_instances) = class_instances
        .iter()
        .filter(|(class, _)| class.starts_with("Gui::"))
        .fold((0usize, 0usize), |(classes, instances), (_, count)| {
            (classes + 1, instances + *count)
        });
    assert_eq!((plain_classes, plain_instances), (17, 326));
    assert_eq!((custom_classes, custom_instances), (15, 169));

    // Secondary structural totals: each is a measured count, so a parser that
    // silently skipped a construct would fail here.
    assert_eq!(layouts, 103, "layout count");
    assert_eq!(spacers, 37, "spacer count");
    assert_eq!(custom_widgets, 55, "custom-widget registry entries");
    assert_eq!(connections, 16, "connection count");
    assert_eq!(tab_order, 35, "tab order entries");
    assert_eq!(combo_items, 70, "combo item count");
    assert_eq!(tab_pages, 2, "tab page count");
    assert_eq!(resources, 1, "resource include count");

    let all_properties: BTreeSet<String> = forms
        .iter()
        .flat_map(|(_, form)| property_names(form))
        .collect();
    assert_eq!(all_properties.len(), 52, "distinct property names");

    for (name, form) in &forms {
        if CORE_FORMS.contains(&name.as_str()) {
            let count = property_names(form).len();
            assert!(
                (12..=16).contains(&count),
                "{name}: expected 12-16 distinct property names, found {count}"
            );
        }
    }

    // The six core forms report 39/15/19/15/26/72 widgets. `FORMS` already pins
    // those numbers; this loop names the acceptance criterion explicitly.
    for (name, widgets) in [
        ("TaskPanel_OrthoArray.ui", 39usize),
        ("TaskPanel_PolarArray.ui", 15),
        ("TaskPanel_CircularArray.ui", 19),
        ("TaskShapeString.ui", 15),
        ("TaskSelectPlane.ui", 26),
        ("TaskPanel_SetStyle.ui", 72),
    ] {
        let form = &forms.iter().find(|(file, _)| file == name).unwrap().1;
        assert_eq!(form.widgets.len(), widgets, "{name}: core form widget count");
    }
}
