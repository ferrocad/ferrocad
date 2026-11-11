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
        // registry — and the script answers. On CPython it reports "nothing selected";
        // here it refuses, because this half has no `FreeCADGui.Selection` mirror
        // (B18a step 4) and no write path to the document (D15, the wasm document is
        // read-only). The difference is the step that is still off, not a missing
        // command registry.
        let delete = host
            .run_command("App_Delete")
            .expect("the registry lists it, so the script half answers it");
        assert!(!delete.ok, "{delete:?}");
        assert!(
            delete.error.contains("Selection"),
            "the refusal names the missing mirror: {}",
            delete.error
        );
    });
}

/// A predicate this half cannot evaluate must not take a verb away, and a Gui host must
/// get the settings *card* rather than the headless modal.
#[test]
fn an_unevaluable_predicate_does_not_disable_a_command() {
    with_host(|host| {
        let commands = host.commands();
        // `App_Undo`/`App_Redo` read `Document.UndoCount`; `App_ToggleVisibility` reads
        // `FreeCADGui.Selection`. The facade has neither yet (B18a step 4), so their
        // `IsActive` raises. Answering `enabled: false` would override the document
        // half's own state — the shell merges the script's row over the catalogue's —
        // and take away verbs the window can still run.
        for id in [catalog::UNDO, catalog::REDO, catalog::TOGGLE_VISIBILITY] {
            let row = commands
                .iter()
                .find(|command| command.id == id)
                .unwrap_or_else(|| panic!("{id} is a registry row: {:?}", commands.len()));
            assert!(
                row.enabled,
                "{id} is not disabled by a predicate this half cannot evaluate"
            );
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
