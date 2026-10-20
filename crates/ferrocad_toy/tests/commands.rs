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

    // Invoking it is the shell's path: run, then re-read the model and the stage.
    let outcome = python::run_command("Toy_MakeSlab").expect("the bridge returned");
    assert!(outcome.ok, "command failed: {}", outcome.error);
    assert_eq!(outcome.status, "created a slab");
    assert_eq!(slab_count(), 2, "the command created a slab");

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
