//! A0 + A1 end to end: the toy workbench's command, through the registry.
//!
//! The command is registered by the app script on `FreeCADGui`; the shell reads it
//! through `ferrocad_shell.commands()` and invokes it through `run_command`. No
//! kernel is involved.

use std::sync::{Mutex, Once};

use ferrocad_gpui::python;

/// The tests share one interpreter and one document registry.
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

fn slab_count() -> usize {
    python::stage_meshes()
        .expect("stage meshes")
        .iter()
        .filter(|mesh| mesh.object.starts_with("Slab"))
        .count()
}

#[test]
fn the_toy_command_is_listed_and_creates_an_object() {
    let _guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    boot();
    assert_eq!(slab_count(), 1, "bootstrap seeds one slab");

    // The bar's input: the command and its live state.
    let commands = python::commands().expect("the command list");
    let command = commands
        .iter()
        .find(|command| command.id == "Toy_MakeSlab")
        .expect("Toy_MakeSlab is registered");
    assert_eq!(command.label(), "Create Slab");
    assert_eq!(command.group, "Toy");
    assert_eq!(command.cmd_type, "ForEdit");
    assert!(command.enabled, "the command is active with a document");
    assert!(!command.checked);

    // Invoking it is the shell's path: the command asks first, then the callback
    // creates the object (A3).
    let outcome = python::run_command("Toy_MakeSlab").expect("the bridge returned");
    assert!(outcome.ok, "command failed: {}", outcome.error);
    let spec = outcome.dialog.clone().expect("a dialog spec");
    assert_eq!(spec.title, "Create Slab");
    assert_eq!(spec.fields.len(), 3);
    let token = outcome.token.clone().expect("a dialog token");
    assert_eq!(
        slab_count(),
        1,
        "nothing is created before the dialog is accepted"
    );

    let values = serde_json::json!({"Width": 50.0, "Depth": 40.0, "Height": 25.0});
    let accepted = ferrocad_gpui::commands::accept(&token, &values).expect("the bridge returned");
    assert!(accepted.ok, "accept failed: {}", accepted.error);
    assert!(
        accepted.status.starts_with("created"),
        "status: {}",
        accepted.status
    );
    assert_eq!(slab_count(), 2, "the callback created the slab");

    // A token is single use.
    let again = ferrocad_gpui::commands::accept(&token, &values).expect("the bridge returned");
    assert!(!again.ok, "a spent token is rejected");
    assert!(
        again.error.contains("no open dialog"),
        "error: {}",
        again.error
    );

    // One command, one transaction: a single undo removes it again.
    let undo = python::evaluate("doc.undo()").expect("undo ran");
    assert!(undo.ok, "undo failed: {}", undo.output);
    assert_eq!(slab_count(), 1, "undo removed the created slab");

    // An unknown id is an error result, not a panic.
    let unknown = python::run_command("No_Such_Command").expect("the bridge returned");
    assert!(!unknown.ok);
    assert!(
        unknown.error.contains("No_Such_Command"),
        "the error names the id: {}",
        unknown.error
    );
}
