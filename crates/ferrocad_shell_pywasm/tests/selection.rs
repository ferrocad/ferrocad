//! B18a step 4: the wasm facade grows the selection mirror, and undo/redo reach.
//!
//! `bootstrap.py` now ports `python/FreeCADGui/__init__.py`'s `Selection`: the shell
//! pushes its snapshot down (`ferrocad_shell.set_selection`) and drains what a script
//! wrote (`take_selection_requests`). The document payload carries `undo_count`/
//! `redo_count`, so `Document.UndoCount`/`RedoCount` answer it. These tests drive the
//! unmodified `python/ferrocad_shell` through the shared host, so they fail if the
//! facade and the script stop agreeing.

use ferrocad_core::{Document, Property, Quantity};
use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::catalog;
use ferrocad_shell_api::commands::CommandInfo;
use ferrocad_shell_api::selection::SelectionItem;
use ferrocad_shell_api::types::SelectionRequest;
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

/// The same, over a payload the test built with `ferrocad_doc_api`'s presenter — the one
/// `snapshot.rs` uses, so `Document.UndoCount` is the engine's answer and not a stub.
fn with_payload<R: Send + 'static>(
    payload: String,
    f: impl FnOnce(&PywasmScriptHost) -> R + Send + 'static,
) -> R {
    std::thread::Builder::new()
        .stack_size(16 << 20)
        .spawn(move || {
            let host = ferrocad_shell_pywasm::over(payload).expect("the interpreter boots");
            f(&host)
        })
        .expect("a worker thread")
        .join()
        .expect("no panic")
}

fn command<'a>(commands: &'a [CommandInfo], id: &str) -> &'a CommandInfo {
    commands
        .iter()
        .find(|command| command.id == id)
        .unwrap_or_else(|| {
            panic!(
                "{id} is a registry row: {:?}",
                commands.iter().map(|c| &c.id).collect::<Vec<_>>()
            )
        })
}

/// A document with one committed transaction, optionally undone, as a payload.
fn history(undo: bool) -> String {
    let mut document = Document::new();
    let object = document.add_object("Box", "Part::Box");
    document.open_transaction_named("Move");
    document
        .set_property(
            object,
            "Length",
            Property::Quantity(Quantity::parse("12 mm").expect("a quantity")),
        )
        .expect("Length on a fresh box");
    document.commit_transaction();
    if undo {
        assert!(document.undo(), "the transaction is undone");
    }
    ferrocad_doc_api::payload::render("History", &document)
}

/// The shell's push is what a command's `IsActive` reads, so the rows are *judged* now:
/// enabled with a selection, disabled without.
#[test]
fn a_selection_backed_predicate_is_judged() {
    with_host(|host| {
        let commands = host.commands();
        for id in [catalog::TOGGLE_VISIBILITY, "App_Delete", catalog::MOVE] {
            assert!(
                !command(&commands, id).enabled,
                "{id} is disabled with nothing selected"
            );
        }

        // Exactly what a click does: `on_selection_changed` → `set_selection` →
        // `FreeCADGui.Selection._set`.
        host.on_selection_changed(&[SelectionItem::new("Box")]);

        let commands = host.commands();
        for id in [catalog::TOGGLE_VISIBILITY, "App_Delete", catalog::MOVE] {
            assert!(
                command(&commands, id).enabled,
                "{id} answers for the shell's selection"
            );
        }

        // And the script reads the same snapshot, through the real shell module.
        // (`evaluate` echoes the value of an expression, so the import is its own
        // line: the namespace it leaves behind is shared by the next call.)
        let imported = host.evaluate("import FreeCADGui").expect("evaluate runs");
        assert!(imported.ok, "{}", imported.output);
        let answer = host
            .evaluate("FreeCADGui.Selection.getSelection()[0].Name")
            .expect("evaluate runs");
        assert!(answer.ok, "{}", answer.output);
        assert_eq!(answer.output, "'Box'");

        // A new push replaces the snapshot: nothing selected is disabled again.
        host.on_selection_changed(&[]);
        assert!(
            !command(&host.commands(), catalog::TOGGLE_VISIBILITY).enabled,
            "the predicate follows the shell, both ways"
        );
    });
}

/// A script's writes update the local snapshot **and** queue a record the shell drains,
/// in order, with the sub-elements and picked points it supplied.
#[test]
fn a_scripted_selection_is_queued_for_the_shell() {
    with_host(|host| {
        let script = host
            .evaluate(
                "import FreeCADGui\n\
                 FreeCADGui.Selection.addSelection(\"Box\")\n\
                 FreeCADGui.Selection.addSelection(\"Fillet\", \"Edge1\", [[1.0, 2.0, 3.0]])\n\
                 FreeCADGui.Selection.clearSelection()\n",
            )
            .expect("evaluate runs");
        assert!(script.ok, "{}", script.output);

        let requests = host.take_selection_requests();
        assert_eq!(requests.len(), 3, "{requests:?}");
        assert_eq!(
            requests[0],
            SelectionRequest {
                op: "add".into(),
                object: "Box".into(),
                elements: Vec::new(),
                points: Vec::new(),
            }
        );
        assert_eq!(
            requests[1],
            SelectionRequest {
                op: "add".into(),
                object: "Fillet".into(),
                elements: vec!["Edge1".into()],
                points: vec![(1.0, 2.0, 3.0)],
            },
            "the sub-element and the picked point travel with the write"
        );
        assert_eq!(requests[2].op, "clear");

        // A drain, not a read: the shell has applied these, so a second call is empty.
        assert!(
            host.take_selection_requests().is_empty(),
            "draining empties the queue"
        );
    });
}

/// `App_Delete` with nothing selected is the script's own early-out (the CPython
/// answer); with a selection it runs into the read-only document instead. Neither is
/// a missing `FreeCADGui.Selection` any more.
#[test]
fn app_delete_refuses_on_the_write_path_not_the_mirror() {
    with_host(|host| {
        let idle = host
            .run_command("App_Delete")
            .expect("the registry lists App_Delete");
        assert!(idle.ok, "{idle:?}");
        assert!(idle.status.contains("nothing selected"), "{}", idle.status);

        host.on_selection_changed(&[SelectionItem::new("Box")]);
        let refused = host
            .run_command("App_Delete")
            .expect("still the registry's command");
        assert!(!refused.ok, "{refused:?}");
        assert!(
            refused.error.contains("openTransaction"),
            "the refusal names the missing write path: {}",
            refused.error
        );
        assert!(
            !refused.error.contains("Selection"),
            "the mirror answered, so it cannot be the reason: {}",
            refused.error
        );
    });
}

/// `Document.UndoCount`/`RedoCount` come from the payload, so `App_Undo`/`App_Redo` are
/// judged by the document's own history instead of the old optimism.
#[test]
fn undo_and_redo_counts_come_from_the_document_payload() {
    with_payload(history(false), |host| {
        let undo = host
            .evaluate("FreeCAD.ActiveDocument.UndoCount")
            .expect("evaluate runs");
        assert!(undo.ok, "{}", undo.output);
        assert_eq!(undo.output, "1", "one committed transaction");
        let redo = host
            .evaluate("FreeCAD.ActiveDocument.RedoCount")
            .expect("evaluate runs");
        assert!(redo.ok, "{}", redo.output);
        assert_eq!(redo.output, "0");

        let commands = host.commands();
        assert!(
            command(&commands, catalog::UNDO).enabled,
            "there is an undo step, so the row is enabled"
        );
        assert!(
            !command(&commands, catalog::REDO).enabled,
            "there is nothing to redo yet"
        );
    });

    with_payload(history(true), |host| {
        let undo = host
            .evaluate("FreeCAD.ActiveDocument.UndoCount")
            .expect("evaluate runs");
        assert_eq!(undo.output, "0", "the transaction was undone");
        let redo = host
            .evaluate("FreeCAD.ActiveDocument.RedoCount")
            .expect("evaluate runs");
        assert_eq!(redo.output, "1");

        let commands = host.commands();
        assert!(!command(&commands, catalog::UNDO).enabled);
        assert!(
            command(&commands, catalog::REDO).enabled,
            "the undone transaction is what redo would reapply"
        );
    });
}
