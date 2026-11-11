//! B18a steps 1–2: the wasm facade answers the shell's view and command groups.
//!
//! `bootstrap.py` grew the pieces the shell module's *own* commands need —
//! `FerroCAD.Grid`/`Axis`/`Gizmo`, `FerroCADGui.ViewState`, `gridViewRequest`/
//! `axisViewRequest`, and the `FreeCADGui` command-registry helpers — and the runtime's
//! `Contract` now says so. These tests drive the unmodified `python/ferrocad_shell`
//! through the shared host, so they fail if the facade and the script stop agreeing.

use ferrocad_shell_api::app::ScriptHost;
use ferrocad_shell_api::catalog::{self, Executor};
use ferrocad_shell_api::view::ViewState;
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

#[test]
fn app_toggle_grid_flips_the_grid_and_returns_the_request() {
    with_host(|host| {
        let on = host
            .run_command("App_ToggleGrid")
            .expect("the script owns the grid toggle");
        assert!(on.ok, "{}", on.error);
        let view = on.view.expect("the toggle returns a view request");
        assert_eq!(view.grid, Some(true), "the first toggle shows the grid");
        // `FerroCAD.Grid.apply()`'s composition, not a hand-made `{grid: true}`: the
        // steps, the style and the ink travel with the switch.
        assert_eq!(view.grid_minor, Some(10.0));
        assert_eq!(view.grid_major, Some(100.0));
        assert_eq!(view.grid_adaptive, Some(true));
        assert_eq!(view.grid_minor_style.as_deref(), Some("dots"));
        assert_eq!(view.grid_color.as_deref(), Some("#94a3b8"));
        assert_eq!(view.grid_alpha, Some(1.0));
        assert_eq!(
            view.grid_overlay, None,
            "no provider is registered, so nothing claims the drawn grid"
        );

        let off = host
            .run_command("App_ToggleGrid")
            .expect("still the script's");
        assert!(off.ok, "{}", off.error);
        assert_eq!(
            off.view.expect("a request").grid,
            Some(false),
            "a second toggle hides it again"
        );
    });
}

#[test]
fn app_toggle_axis_cross_flips_the_axis_request() {
    with_host(|host| {
        let first = host
            .run_command("App_ToggleAxisCross")
            .expect("the script owns the axis toggle");
        assert!(first.ok, "{}", first.error);
        let view = first.view.expect("the toggle returns a view request");
        // `FerroCAD.Axis.enabled` starts True (`_axis.DEFAULT_ENABLED`), so the first
        // toggle is the one that turns the triad off.
        let axis = view.axis.expect("the axis policy travels in `view.axis`");
        assert_eq!(axis.enabled, Some(false), "{axis:?}");
        assert_eq!(
            axis.overlay, None,
            "no provider is registered, so nothing claims the drawn triad"
        );
        assert_eq!(view.axis_cross, None, "the nested request carries the flag");

        let second = host
            .run_command("App_ToggleAxisCross")
            .expect("still the script's");
        assert!(second.ok, "{}", second.error);
        let axis = second
            .view
            .expect("a request")
            .axis
            .expect("the axis policy travels in `view.axis`");
        assert_eq!(axis.enabled, Some(true), "and a second turns it back on");
    });
}

#[test]
fn a_pushed_view_state_is_what_the_next_toggle_acts_on() {
    with_host(|host| {
        let mut state = ViewState::default();
        state.grid = true;
        host.on_view_state_changed(&state);

        // The facade has no parameter store, so an unwritten `FerroCAD.Grid.enabled`
        // reads the shell's mirror — which is why the toggle turns *off* a grid the
        // shell says it is drawing, instead of toggling a grid that is already on.
        let outcome = host
            .run_command("App_ToggleGrid")
            .expect("the script owns the toggle");
        assert!(outcome.ok, "{}", outcome.error);
        assert_eq!(
            outcome.view.expect("a request").grid,
            Some(false),
            "the push reached the command"
        );
    });
}

#[test]
fn the_appearance_policy_is_the_three_capabilities_request() {
    with_host(|host| {
        let outcome = host
            .appearance_view()
            .expect("the script owns the boot appearance hook");
        assert!(outcome.ok, "{} {}", outcome.error, outcome.status);
        let view = outcome.view.expect("the hook returns a view request");
        // The native facade merges `FerroCAD.Gizmo`/`Axis`/`Grid` over an empty
        // preset and provider registry; these are their declared defaults.
        assert_eq!(view.grid, Some(false));
        let axis = view.axis.clone().expect("the axis policy");
        assert_eq!(axis.enabled, Some(true));
        assert_eq!(axis.color_x.as_deref(), Some("#f87171"));
        let gizmo = view.gizmo.clone().expect("the gizmo policy");
        assert_eq!(gizmo.backend.as_deref(), Some("ferrocad"));
        assert_eq!(gizmo.show_readout, Some(true));

        // And the shell can apply it: the boot hook's request goes through the same
        // `ViewState::apply` a command's does, so a field outside its bounds would
        // make the whole appearance request a no-op.
        let mut state = ViewState::default();
        let summary = state.apply(&view).expect("the shell accepts the request");
        assert!(!state.grid, "{summary}");
    });
}

#[test]
fn the_registry_lists_the_shells_own_commands() {
    with_host(|host| {
        let commands = host.commands();

        // Every id the catalogue hands to the script is one the shell module actually
        // registered at import: `run_command` routes the `Executor::Script` group here
        // whatever the registry says, so a missing registration would be a refusal at
        // run time rather than a fallback to the document half.
        let script_ids: Vec<&str> = catalog::builtin()
            .iter()
            .map(|def| def.id)
            .filter(|id| catalog::executor(id) == Executor::Script)
            .collect();
        for id in [
            catalog::TOGGLE_GRID,
            catalog::VIEW_GRID_SETTINGS,
            catalog::TOGGLE_AXIS_CROSS,
            catalog::VIEW_WORKPLANE,
            catalog::TOGGLE_GIZMO,
            catalog::DEV_TOOLS,
        ] {
            assert!(
                script_ids.contains(&id),
                "{id} is a catalogue Script id: {script_ids:?}"
            );
            assert!(
                commands.iter().any(|command| command.id == id),
                "{id} is registered by the shell module: {:?}",
                commands.iter().map(|c| &c.id).collect::<Vec<_>>()
            );
        }

        let grid = commands
            .iter()
            .find(|command| command.id == "App_ToggleGrid")
            .expect("App_ToggleGrid is listed");
        assert_eq!(
            grid.surface, "grid",
            "the catalogue's surface metadata came through `GetResources`"
        );
        assert!(grid.checkable, "`IsChecked` exists, so the row is a toggle");

        // `App_Delete` is not in the catalogue, so `run_command` routes it to the
        // registry — and the script answers what CPython answers with nothing
        // selected. Since B18a step 4 grew `FreeCADGui.Selection`, both the predicate
        // and the command have the state they read; the read-only document is
        // `tests/selection.rs`'s row, because reaching it needs a selection.
        let delete = host
            .run_command("App_Delete")
            .expect("the registry lists it, so the script half answers it");
        assert!(delete.ok, "{delete:?}");
        assert!(
            delete.status.contains("nothing selected"),
            "the script's own early-out, not a missing mirror: {}",
            delete.status
        );
    });
}

/// A predicate is judged when this half has its state; it keeps the document half's
/// answer only when it asks for a capability this half has not grown; and a predicate
/// that is simply broken disables its row, exactly as the real facade's does.
///
/// A Gui host must also get the settings *card* rather than the headless modal.
#[test]
fn a_broken_predicate_disables_a_command_but_an_absent_capability_does_not() {
    with_host(|host| {
        let setup = host
            .evaluate(
                r#"import FreeCADGui
class Broken:
    def GetResources(self):
        return {"MenuText": "Broken"}

    def IsActive(self):
        raise ValueError("the predicate itself is wrong")

class Absent:
    def GetResources(self):
        return {"MenuText": "Absent"}

    def IsActive(self):
        return FreeCAD.ParamGet("User parameter:BaseApp/Preferences/Test") is not None

FreeCADGui.addCommand("Test_Broken", Broken())
FreeCADGui.addCommand("Test_Absent", Absent())
"#,
            )
            .expect("evaluate runs");
        assert!(setup.ok, "{}", setup.output);

        let commands = host.commands();
        let find = |id: &str| {
            commands
                .iter()
                .find(|command| command.id == id)
                .unwrap_or_else(|| {
                    panic!(
                        "{id} is a registry row: {:?}",
                        commands.iter().map(|c| &c.id).collect::<Vec<_>>()
                    )
                })
        };

        // The real facade disables a command whose `IsActive` raises: the predicate is
        // broken, and running it would fail too.
        assert!(
            !find("Test_Broken").enabled,
            "a predicate that raises for its own reasons disables its row"
        );
        // `FreeCAD.ParamGet` is B18b: the raise means the capability is absent, not that
        // the command is dead, so the document half's `enabled` stands.
        assert!(
            find("Test_Absent").enabled,
            "an absent capability is not a dead command"
        );

        // The built-in rows are judged now, not optimistic: the sample has no history
        // and nothing is selected, so all three are genuinely disabled.
        for id in [catalog::UNDO, catalog::REDO, catalog::TOGGLE_VISIBILITY] {
            assert!(!find(id).enabled, "{id} is judged by the state it reads");
        }

        // A window is attached (`FreeCAD.GuiUp == 1`), so this selects the tree card,
        // which is the shell's one grid-settings surface (B13b) — not the modal that
        // exists for a console-only run.
        let settings = host
            .run_command(catalog::VIEW_GRID_SETTINGS)
            .expect("the script owns the grid settings");
        assert!(settings.ok, "{settings:?}");
        assert_eq!(settings.select.as_deref(), Some("Settings/Grid"));
    });
}
