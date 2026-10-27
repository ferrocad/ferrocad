"""User workflow: turn the reference grid on and off, and read the switch back."""

import FerroCAD
import FerroCADGui

from _capture import main

QUADRANT = "how-to"
SLUG = "grid-visibility"
TITLE = "Turn the reference grid on and off"
SUMMARY = (
    "The Grid command is checkable and presentation-only. This page runs it with no "
    "window, shows the view request it returns, and reads the one switch every grid "
    "backend agrees on."
)
REQUIRES = ["the `FerroCAD` facade and `ferrocad_shell`; no GPU and no window"]


def run(cap):
    with cap.step("Start from the default"):
        cap.prose(
            "`FerroCAD.Grid` is the semantic settings object for the reference grid. "
            "`enabled` is the switch a provider reads; it defaults to off."
        )
        cap.exec(
            """
            import json
            import ferrocad_shell

            FerroCAD.Grid.reset()
            """
        )
        cap.result("FerroCAD.Grid.enabled", FerroCAD.Grid.enabled)

    with cap.step("Run the command"):
        cap.prose(
            "`App_ToggleGrid` is registered by the shell and is checkable. It flips "
            "`FerroCAD.Grid.enabled` and returns a `view` request; the shell applies "
            "that request and mirrors the result into `FerroCADGui.ViewState`."
        )
        cap.exec(
            """
            first = json.loads(ferrocad_shell.run_command("App_ToggleGrid"))
            """
        )
        cap.result("Response", first)
        cap.result("FerroCAD.Grid.enabled", FerroCAD.Grid.enabled)
        cap.image(
            "grid-visibility.png",
            "The Toy edition with the reference grid visible",
            caption=(
                "The Toy edition with the grid on. The `Grid` control in the header "
                "reads the same switch `App_ToggleGrid` flips."
            ),
        )

    with cap.step("Push the state the window would push"):
        cap.prose(
            "With no window, nothing has mirrored the request yet, so the check mark "
            "still reads the old state. `set_view_state` is the call the shell makes "
            "after it applies a request."
        )
        cap.exec(
            """
            ferrocad_shell.set_view_state(json.dumps(first["view"]))
            checked = {
                c["id"]: c["checked"]
                for c in json.loads(ferrocad_shell.commands())
                if c["id"] == "App_ToggleGrid"
            }
            """
        )
        cap.result("App_ToggleGrid checked", checked)
        cap.result("FerroCADGui.ViewState.grid", FerroCADGui.ViewState.grid)

    with cap.step("Turn it off again"):
        cap.exec(
            """
            second = json.loads(ferrocad_shell.run_command("App_ToggleGrid"))
            ferrocad_shell.set_view_state(json.dumps(second["view"]))
            """
        )
        cap.result("Response", second)
        cap.result("FerroCAD.Grid.enabled", FerroCAD.Grid.enabled)
        cap.note(
            "Every grid backend reads this one switch. A provider that kept its own "
            "copy would keep drawing after the toggle, which is exactly the defect "
            "`docs/project/record/tasks/grid-visibility.md` fixed."
        )


if __name__ == "__main__":
    main(
        run,
        slug=SLUG,
        quadrant=QUADRANT,
        title=TITLE,
        summary=SUMMARY,
        requires=REQUIRES,
    )
