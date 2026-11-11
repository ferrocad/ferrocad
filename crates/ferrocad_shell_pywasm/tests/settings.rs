//! B18a step 3: the wasm facade's `Settings/<Feature>` cards.
//!
//! `bootstrap.py` ports `python/FerroCADGui/__init__.py`'s settings half:
//! `FerroCADGui.settings`/`setSetting`/`resetAppearance` over the provider schema the
//! registry stores. The host exposes it through `extra_properties`/`set_extra_property`,
//! so these tests drive the unmodified `python/ferrocad_shell` through the shared host —
//! the same calls the property pane makes for a `Settings/<Feature>` node.
//!
//! The store is in memory (no parameter store: B18b), and the coercion, ranges and
//! choices are the native ones, including the quantity unit rule this build can honour
//! without `FreeCAD.Units`.

use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::types::PropRow;
use ferrocad_shell_pywasm::PywasmScriptHost;

/// Run `f` against a freshly booted interpreter on a large stack (RustPython's compiler
/// recurses; see `properties.rs` for the same requirement).
fn with_host<R: Send + 'static>(f: impl FnOnce(&PywasmScriptHost) -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let host = ferrocad_shell_pywasm::new().expect("the interpreter boots");
            f(&host)
        })
        .expect("a worker thread")
        .join()
        .expect("no panic")
}

/// Run Python and assert it did not raise; returns the console output.
fn evaluate(host: &PywasmScriptHost, code: &str) -> String {
    let result = host.evaluate(code).expect("evaluate runs");
    assert!(result.ok, "{}", result.output);
    result.output
}

/// One row of the card, by name.
fn row<'a>(rows: &'a [PropRow], name: &str) -> &'a PropRow {
    rows.iter().find(|row| row.name == name).unwrap_or_else(|| {
        panic!(
            "{name} is not a row: {:?}",
            rows.iter().map(|row| &row.name).collect::<Vec<_>>()
        )
    })
}

/// The card for a provider: the selector row plus the schema's rows, each reading its
/// declared default, and a write that comes back on the next read.
#[test]
fn a_provider_card_lists_the_schema_and_round_trips_a_write() {
    with_host(|host| {
        evaluate(
            host,
            r#"import FerroCADGui
FerroCADGui.registerProvider('grid', 'demo', 'Demo Grid', settings=[
    {'name': 'minor', 'kind': 'int', 'min': 1, 'max': 100, 'default': 10},
    {'name': 'style', 'kind': 'enum', 'choices': ['dots', 'lines'], 'default': 'dots'},
    {'name': 'enabled', 'kind': 'bool', 'default': True},
    {'name': 'ink', 'kind': 'color', 'default': '#94a3b8'},
    {'name': 'spacing', 'kind': 'quantity', 'unit': 'mm', 'default': 10.0},
])
"#,
        );

        let rows = host
            .extra_properties("Settings/grid")
            .expect("the script owns the grid card");
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names[0], "provider", "the selector comes first: {names:?}");
        assert!(names.contains(&"minor"), "{names:?}");
        assert_eq!(row(&rows, "provider").value, "Demo Grid");
        assert_eq!(row(&rows, "provider").kind, "enum");
        assert_eq!(row(&rows, "provider").choices, ["Demo Grid"]);
        assert_eq!(row(&rows, "minor").value, "10");
        assert_eq!(row(&rows, "minor").min, Some(1.0));
        assert_eq!(row(&rows, "minor").max, Some(100.0));
        assert_eq!(row(&rows, "minor").kind, "int");
        assert!(row(&rows, "minor").editable);
        assert_eq!(row(&rows, "style").value, "dots");
        assert_eq!(row(&rows, "style").choices, ["dots", "lines"]);
        assert_eq!(row(&rows, "enabled").value, "True");
        assert_eq!(row(&rows, "ink").value, "#94a3b8");
        assert_eq!(row(&rows, "spacing").value, "10 mm");
        assert_eq!(row(&rows, "spacing").unit.as_deref(), Some("mm"));
        assert!(row(&rows, "spacing").editable);

        // The edit the pane makes: the text field's string, on the row's name.
        let written = host
            .set_extra_property("Settings/grid", "minor", "25")
            .expect("the script owns the card");
        assert!(written.ok, "{written:?}");
        let rows = host.extra_properties("Settings/grid").expect("the card");
        assert_eq!(row(&rows, "minor").value, "25");

        // A quantity takes the unit's text and a bare number; a different unit is
        // refused rather than mis-scaled (there is no `FreeCAD.Units` here).
        assert!(
            host.set_extra_property("Settings/grid", "spacing", "12.5 mm")
                .expect("the card")
                .ok
        );
        let rows = host.extra_properties("Settings/grid").expect("the card");
        assert_eq!(row(&rows, "spacing").value, "12.5 mm");
        assert!(
            host.set_extra_property("Settings/grid", "spacing", "20")
                .expect("the card")
                .ok
        );
        let rows = host.extra_properties("Settings/grid").expect("the card");
        assert_eq!(row(&rows, "spacing").value, "20 mm");
    });
}

/// A value the schema rejects is a result naming the row, and the store keeps the old
/// value. This is the same rule the native facade applies, so a card cannot diverge.
#[test]
fn a_rejected_value_changes_nothing_and_names_the_row() {
    with_host(|host| {
        evaluate(
            host,
            r#"import FerroCADGui
FerroCADGui.registerProvider('grid', 'demo', 'Demo Grid', settings=[
    {'name': 'minor', 'kind': 'int', 'min': 1, 'max': 100, 'default': 10},
    {'name': 'style', 'kind': 'enum', 'choices': ['dots', 'lines'], 'default': 'dots'},
    {'name': 'ink', 'kind': 'color', 'default': '#94a3b8'},
    {'name': 'spacing', 'kind': 'quantity', 'unit': 'mm', 'default': 10.0},
])
"#,
        );

        for (name, value, expected) in [
            ("minor", "0", "at least 1"),
            ("minor", "101", "at most 100"),
            ("minor", "2.5", "expects an integer"),
            ("style", "zigzag", "must be one of"),
            ("ink", "red", "#rrggbb"),
            ("spacing", "20 cm", "expects a quantity"),
            ("nobody", "1", "no setting 'nobody'"),
        ] {
            let refused = host
                .set_extra_property("Settings/grid", name, value)
                .expect("the card");
            assert!(!refused.ok, "{name}={value} should be refused: {refused:?}");
            assert!(
                refused.error.contains(expected),
                "{name}={value}: {}",
                refused.error
            );
        }
        let rows = host.extra_properties("Settings/grid").expect("the card");
        assert_eq!(row(&rows, "minor").value, "10", "nothing was written");
        assert_eq!(row(&rows, "style").value, "dots");
        assert_eq!(row(&rows, "ink").value, "#94a3b8");
        assert_eq!(row(&rows, "spacing").value, "10 mm");
    });
}

/// A `get`/`set` pair owns the value: the hook is what a write runs and what a read
/// asks. A `get` without a `set` is read-only, which the pane must see as such.
#[test]
fn a_hook_pair_owns_the_value_and_a_getter_alone_is_read_only() {
    with_host(|host| {
        evaluate(
            host,
            r#"import FreeCAD, FerroCADGui
FreeCAD.demo_state = {'gain': 3}
FerroCADGui.registerProvider('gizmo', 'demo', 'Demo Gizmo', settings=[
    {'name': 'gain', 'kind': 'int', 'default': 1,
     'get': lambda: FreeCAD.demo_state['gain'],
     'set': lambda value: FreeCAD.demo_state.__setitem__('gain', value)},
    {'name': 'computed', 'kind': 'int', 'default': 7, 'get': lambda: 42},
])
"#,
        );

        let rows = host.extra_properties("Settings/gizmo").expect("the card");
        assert_eq!(row(&rows, "gain").value, "3", "the getter is asked first");
        assert_eq!(row(&rows, "gain").status, "computed");
        assert!(row(&rows, "gain").editable, "get and set: editable");
        assert_eq!(row(&rows, "computed").value, "42");
        assert!(
            !row(&rows, "computed").editable,
            "a getter without a setter"
        );

        let written = host
            .set_extra_property("Settings/gizmo", "gain", "9")
            .expect("the card");
        assert!(written.ok, "{written:?}");
        let rows = host.extra_properties("Settings/gizmo").expect("the card");
        assert_eq!(row(&rows, "gain").value, "9");

        let refused = host
            .set_extra_property("Settings/gizmo", "computed", "1")
            .expect("the card");
        assert!(!refused.ok, "{refused:?}");
        assert!(refused.error.contains("read-only"), "{}", refused.error);
    });
}

/// The reset row the card appends for an appearance feature resets the **edition's**
/// store, not a provider's own rows: that is what `resetAppearance` means natively
/// (`App_ResetGridAppearance`), and the shared facade owns it now. The row exists because
/// the shell module registers that command.
#[test]
fn the_reset_row_restores_the_edition_store() {
    with_host(|host| {
        evaluate(
            host,
            r#"import FerroCADGui
FerroCADGui.registerProvider('grid', 'demo', 'Demo Grid', settings=[
    {'name': 'minor', 'kind': 'int', 'default': 10},
])
"#,
        );
        // The provider's row and the edition's own `FerroCAD.Grid` store are separate;
        // the card's reset action is about the latter.
        assert!(
            host.set_extra_property("Settings/grid", "minor", "40")
                .expect("the card")
                .ok
        );
        evaluate(host, "import FerroCAD\nFerroCAD.Grid.minor = 33");
        assert_eq!(evaluate(host, "FerroCAD.Grid.minor"), "33.0");

        let rows = host.extra_properties("Settings/grid").expect("the card");
        assert!(
            rows.iter()
                .any(|row| row.name == "reset" && row.command.is_some()),
            "the reset action row is on the card: {:?}",
            rows.iter().map(|row| &row.name).collect::<Vec<_>>()
        );

        let reset = host
            .set_extra_property("Settings/grid", "reset", "")
            .expect("the card");
        assert!(reset.ok, "{reset:?}");
        assert_eq!(
            evaluate(host, "FerroCAD.Grid.minor"),
            "10.0",
            "the edition store is back to its default"
        );
        let rows = host.extra_properties("Settings/grid").expect("the card");
        assert_eq!(
            row(&rows, "minor").value,
            "40",
            "a provider's own row is not the edition's store"
        );
    });
}

/// A write returns the provider's `refresh` claim, and the provider selector row
/// switches the backend — both are what the pane applies and what the header repaints
/// from.
#[test]
fn a_write_carries_the_refresh_claim_and_the_selector_switches() {
    with_host(|host| {
        evaluate(
            host,
            r#"import FerroCADGui
def refresh():
    return {'view': {'grid_overlay': True}, 'status': 'demo read it back'}
FerroCADGui.registerProvider('grid', 'alpha', 'Alpha', refresh=refresh, settings=[
    {'name': 'minor', 'kind': 'int', 'default': 10},
])
FerroCADGui.registerProvider('grid', 'beta', 'Beta', settings=[
    {'name': 'minor', 'kind': 'int', 'default': 20},
])
"#,
        );

        let written = host
            .set_extra_property("Settings/grid", "minor", "30")
            .expect("the card");
        assert!(written.ok, "{written:?}");
        assert_eq!(
            written.view.and_then(|view| view.grid_overlay),
            Some(true),
            "the refresh callback's claim reaches the shell"
        );

        // Switching through the row the card shows keeps each backend's own value.
        let switched = host
            .set_extra_property("Settings/grid", "provider", "Beta")
            .expect("the card");
        assert!(switched.ok, "{switched:?}");
        let rows = host.extra_properties("Settings/grid").expect("the card");
        assert_eq!(row(&rows, "provider").value, "Beta");
        assert_eq!(
            evaluate(host, "FerroCADGui.activeProvider('grid')"),
            "'beta'"
        );
        assert_eq!(
            row(&rows, "minor").value,
            "20",
            "beta's own value, not alpha's"
        );

        let switched = host
            .set_extra_property("Settings/grid", "provider", "Alpha")
            .expect("the card");
        assert!(switched.ok, "{switched:?}");
        let rows = host.extra_properties("Settings/grid").expect("the card");
        assert_eq!(row(&rows, "minor").value, "30", "alpha kept its write");
    });
}
