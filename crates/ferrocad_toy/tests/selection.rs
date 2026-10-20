//! A2 end to end: the shell's selection snapshot drives the built-in commands.
//!
//! The test uses the toy edition so no kernel is involved: it pushes a selection
//! through the same bridge the shell uses (`ferrocad_gpui::selection`), reads the
//! command list a real `IsActive` produces, and runs `App_Delete` / `App_Move`.

use std::sync::{Mutex, Once};

use ferrocad_gpui::python;
use ferrocad_gpui::selection::{self, SelectionItem};

/// The tests share one interpreter; serialize them.
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Register the built-in modules and the app script before the interpreter starts.
static BOOT: Once = Once::new();

fn boot() {
    BOOT.call_once(|| {
        ferrocad::register_core_module();
        python::configure("ferrocad_toy", &[], &[], false);
    });
    python::bootstrap().expect("boot the toy app");
}

/// A document with one slab, isolated from whatever the previous test left behind.
fn fresh_slab_document() {
    boot();
    python::evaluate(
        "FreeCAD.closeDocument(doc.Name)\ndoc = FreeCAD.newDocument('A2')\nToy.makeSlab()",
    )
    .expect("a fresh document");
}

fn slab_names() -> Vec<String> {
    python::stage_meshes()
        .expect("stage meshes")
        .iter()
        .map(|mesh| mesh.object.clone())
        .filter(|name| name.starts_with("Slab"))
        .collect()
}

fn command(id: &str) -> ferrocad_gpui::CommandInfo {
    python::commands()
        .expect("the command list")
        .into_iter()
        .find(|command| command.id == id)
        .unwrap_or_else(|| panic!("{id} is registered"))
}

fn placement_x(name: &str) -> f64 {
    let out =
        python::evaluate(&format!("doc.getObject({name:?}).Placement.Base.x")).expect("evaluate");
    assert!(out.ok, "{}", out.output);
    out.output.trim().parse().expect("a number")
}

#[test]
fn the_delete_command_removes_two_selected_slabs_in_one_step() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    fresh_slab_document();
    assert_eq!(slab_names().len(), 1, "the fresh document has one slab");

    // A second slab through the toy's own command: the bar asks for its size first.
    let created = python::run_command("Toy_MakeSlab").expect("the bridge");
    assert!(created.ok, "{}", created.error);
    let token = created.token.expect("a dialog token");
    let values = serde_json::json!({"Width": 40.0, "Depth": 40.0, "Height": 20.0});
    let accepted = ferrocad_gpui::commands::accept(&token, &values).expect("accept");
    assert!(accepted.ok, "{}", accepted.error);
    assert_eq!(slab_names().len(), 2);

    // The shell's path: push the selection, then ask the registry. `IsActive` must
    // have flipped, because the push happens before the list is read.
    let items: Vec<SelectionItem> = slab_names().into_iter().map(SelectionItem::new).collect();
    selection::push(&items).expect("push the selection");
    assert!(
        command("App_Delete").enabled,
        "Delete is enabled by the selection"
    );
    assert!(command("App_Move").enabled);

    let outcome = python::run_command("App_Delete").expect("the bridge");
    assert!(outcome.ok, "{}", outcome.error);
    assert_eq!(outcome.status, "deleted 2 object(s)");
    assert!(slab_names().is_empty(), "both slabs are gone");

    // One command, one transaction: a single undo restores both.
    let undo = python::evaluate("doc.undo()").expect("undo ran");
    assert!(undo.ok, "{}", undo.output);
    assert_eq!(slab_names().len(), 2, "one undo restored both slabs");

    // Clearing the snapshot disables the command again.
    selection::push(&[]).expect("clear the selection");
    assert!(!command("App_Delete").enabled);
}

#[test]
fn the_move_command_translates_the_selection_as_one_step() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    fresh_slab_document();
    let name = slab_names().into_iter().next().expect("the slab");
    selection::push(&[SelectionItem::new(&name)]).expect("push the selection");
    assert!(
        (placement_x(&name)).abs() < 1e-9,
        "the slab starts at the origin"
    );

    // Without a delta the command opens the move dialog; nothing moves until it is
    // accepted, and cancelling leaves the object alone.
    let pending = python::run_command("App_Move").expect("the bridge");
    assert!(pending.ok);
    assert_eq!(pending.dialog.as_ref().expect("a dialog").fields.len(), 3);
    let token = pending.token.expect("a dialog token");
    let cancelled = ferrocad_gpui::commands::cancel(&token).expect("cancel");
    assert!(cancelled.ok);
    assert!(placement_x(&name).abs() < 1e-9, "nothing moved");

    // With a delta it is the non-interactive twin of the gizmo.
    let moved = python::evaluate("import FreeCADGui as Gui\nGui.runCommand('App_Move', 5, 0, 0)")
        .expect("move ran");
    assert!(moved.ok, "{}", moved.output);
    assert!((placement_x(&name) - 5.0).abs() < 1e-9);

    let undo = python::evaluate("doc.undo()").expect("undo ran");
    assert!(undo.ok, "{}", undo.output);
    assert!(
        placement_x(&name).abs() < 1e-9,
        "one undo restored the placement"
    );
}
