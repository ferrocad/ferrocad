//! The S8 acceptance: `python/ferrocad_shell.properties` over the sample document,
//! answered by a Python interpreter running in Rust.
//!
//! The script is the *unmodified* `python/ferrocad_shell/__init__.py`; the rows are
//! the ones `ferrocad_core::present` decides (`kind` and display text). If either
//! side drifts — a renamed property, a new editor kind, a changed format — this test
//! fails, which is the point of running the real script rather than a copy of it.

use ferrocad_shell_pywasm::PywasmScriptHost;
use serde_json::Value;

/// Run `f` against a freshly booted interpreter on a large stack.
///
/// RustPython's compiler and evaluator recurse, and a test thread's default 2 MiB
/// stack is not enough for `python/ferrocad_shell/__init__.py`. The same holds on
/// wasm, where the host must raise the module's stack (or run the interpreter on its
/// own stack) before `PywasmScriptHost::new` — this helper is that requirement,
/// written down where it fails first.
fn with_host<R: Send + 'static>(f: impl FnOnce(&PywasmScriptHost) -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let host = PywasmScriptHost::new().expect("the interpreter boots");
            f(&host)
        })
        .expect("a worker thread")
        .join()
        .expect("no panic")
}

fn property_rows(object: &str) -> Vec<Value> {
    let object = object.to_string();
    with_host(move |host| {
        let json = host.properties(&object).expect("properties() runs");
        serde_json::from_str::<Vec<Value>>(&json).expect("properties() returns a JSON array")
    })
}

/// Find a row by property name.
fn row<'a>(rows: &'a [Value], name: &str) -> &'a Value {
    rows.iter()
        .find(|row| row["name"] == name)
        .unwrap_or_else(|| panic!("{name} is a row; got {:?}", names(rows)))
}

fn names(rows: &[Value]) -> Vec<String> {
    rows.iter()
        .map(|row| row["name"].as_str().unwrap_or("?").to_string())
        .collect()
}

#[test]
fn the_box_reports_the_properties_the_pane_shows() {
    let rows = property_rows("Box");
    let mut names = names(&rows);
    names.sort();
    assert_eq!(
        names,
        ["Height", "Label", "Length", "Placement", "Width"],
        "the sample box's rows (Label first, as the editor shows it)"
    );
}

#[test]
fn a_quantity_is_a_quantity_and_a_float_is_a_float() {
    let box_rows = property_rows("Box");
    // D4/D5: the kind comes from the *value*. A length carries a unit, so it is a
    // quantity; the fillet's radius is a bare float. Neither is decided in Python.
    let length = row(&box_rows, "Length");
    assert_eq!(length["type"], "App::PropertyLength");
    assert_eq!(length["kind"], "quantity");
    assert_eq!(length["value"], "10 mm");
    assert_eq!(length["editable"], true);

    let fillet_rows = property_rows("Fillet");
    let radius = row(&fillet_rows, "Radius");
    assert_eq!(radius["type"], "App::PropertyFloat");
    assert_eq!(radius["kind"], "float");
    assert_eq!(radius["value"], "3.0");
}

#[test]
fn a_placement_row_is_read_only() {
    // `present::kind` has no editor for a placement, so the pane shows the value
    // and offers no control — the script reads that from an empty `kind`.
    let rows = property_rows("Box");
    let placement = row(&rows, "Placement");
    assert_eq!(placement["type"], "App::PropertyPlacement");
    assert_eq!(placement["kind"], "");
    assert_eq!(placement["editable"], false);
}

#[test]
fn an_unknown_object_has_no_rows() {
    // The script's own early-out (`doc.getObject` returning `None`), not a stub.
    assert!(property_rows("Nope").is_empty());
}

#[test]
fn the_console_evaluates_in_the_shells_namespace() {
    // `bootstrap()` has not run, so the namespace is empty: the point is that the
    // interpreter, its frozen `json` and the `io`/`contextlib` capture all work.
    let answer = with_host(|host| host.call("evaluate", "1 + 1").expect("evaluate runs"));
    let value: Value = serde_json::from_str(&answer).expect("evaluate returns JSON");
    assert_eq!(value["ok"], true);
    assert_eq!(value["output"], "2");
}
