"""User workflow: enablement, dialogs, and commands acting on the selection."""

from _capture import main

import FreeCAD
import FreeCADGui

QUADRANT = "how-to"
SLUG = "selection-and-commands"
TITLE = "Drive a command from the selection"
SUMMARY = (
    "Selection is UI state the shell pushes into the facade, and a command reads it "
    "when the shell asks whether it is enabled. This page runs a command with no "
    "selection, selects an object, opens a dialog and accepts it, and deletes the "
    "selection."
)
REQUIRES = [
    "the `ferrocad_toy` app script (it registers on import)",
    "no window: the shell's pushes are ordinary function calls",
]


def run(cap):
    with cap.step("Boot the edition"):
        cap.exec(
            """
            import json
            import ferrocad_shell
            import ferrocad_toy

            ferrocad_toy.bootstrap("Toy")
            """
        )
        cap.result("Objects", [obj.Name for obj in FreeCAD.ActiveDocument.Objects])

    with cap.step("Enablement follows the selection"):
        cap.prose(
            "The shell pushes its selection into `FreeCADGui.Selection`; a command "
            "answers `IsActive` against it. Nothing is enabled or disabled by hand."
        )
        cap.exec("ferrocad_shell.set_selection('[]')")
        cap.exec(
            """
            def delete_state():
                for command in json.loads(ferrocad_shell.commands()):
                    if command["id"] == "App_Delete":
                        return command["enabled"]
            """
        )
        cap.result("App_Delete, nothing selected", delete_state())
        cap.exec('ferrocad_shell.set_selection(\'[{"object": "Slab"}]\')')
        cap.result("Selection", [obj.Name for obj in FreeCADGui.Selection.getSelection()])
        cap.result("App_Delete, one object selected", delete_state())

    with cap.step("A command that asks for input"):
        cap.prose(
            "`Toy_MakeSlab` does not act immediately. It returns a dialog description "
            "and a token; the shell renders the fields natively and sends the typed "
            "values back to that token."
        )
        cap.exec(
            """
            request = json.loads(ferrocad_shell.run_command("Toy_MakeSlab"))
            """
        )
        cap.json("Dialog request", request["dialog"])
        cap.prose("Accepting runs the command's callback, which owns its transaction.")
        cap.exec(
            """
            accepted = json.loads(
                ferrocad_shell.dialog_accept(
                    json.dumps(
                        {
                            "token": request["token"],
                            "values": {"Width": 50.0, "Depth": 25.0, "Height": 12.0},
                        }
                    )
                )
            )
            """
        )
        cap.result("Accept response", accepted)
        cap.result("Objects", [obj.Name for obj in FreeCAD.ActiveDocument.Objects])

    with cap.step("What the command created"):
        cap.prose(
            "`Toy_MakeSlab` created a second slab with the values from the dialog. "
            "The box below is its footprint and height; the document tree above is "
            "the same object."
        )
        cap.exec(
            """
            created = next(
                obj
                for obj in FreeCAD.ActiveDocument.Objects
                if obj.Name.startswith("Slab") and obj.Name != "Slab"
            )
            box_size = (float(created.Width), float(created.Depth), float(created.Height))
            box_center = [value / 2.0 for value in box_size]
            span = max(box_size)
            box_eye = [
                box_center[0] - span * 0.8,
                box_center[1] - span * 1.8,
                box_center[2] + span * 1.5,
            ]
            """
        )
        cap.result("Created object", created.Name)
        cap.result("Box (width, depth, height)", box_size)
        cap.image(
            "selection-and-commands.png",
            "A 50 by 25 by 12 millimetre slab drawn in the viewport",
            caption=(
                "The slab `Toy_MakeSlab` created from the dialog values, drawn by the "
                "same offscreen renderer the viewport uses."
            ),
            shape={"box": {"size": list(box_size), "origin": [0.0, 0.0, 0.0]}},
            color=[0.55, 0.57, 0.60, 1.0],
            camera={
                "eye": box_eye,
                "target": box_center,
                "up": [0.0, 0.0, 1.0],
                "fov_y": 0.6,
            },
        )
        cap.image(
            "selection-and-commands-window.png",
            "The Toy edition with the workflow's document and Dev Tools open",
            caption=(
                "The same document in the Toy edition, with Dev Tools open. "
                "`Slab001` is the object the dialog created, and the Commands list is "
                "the shell's view of `commands()`."
            ),
        )

    with cap.step("Cancelling instead"):
        cap.prose(
            "A token is single-use. Cancelling drops it without running the callback, "
            "so a cancelled dialog leaves no transaction behind."
        )
        cap.exec(
            """
            cancelled = json.loads(ferrocad_shell.run_command("Toy_MakeSlab"))
            cancel_response = json.loads(
                ferrocad_shell.dialog_cancel(cancelled["token"])
            )
            """
        )
        cap.result("Cancel response", cancel_response)
        cap.result("Objects", [obj.Name for obj in FreeCAD.ActiveDocument.Objects])

    with cap.step("Act on the selection"):
        cap.prose(
            "`App_Delete` is the shell's own edit command. It reads the same selection "
            "snapshot and deletes what it names."
        )
        cap.exec(
            """
            ferrocad_shell.set_selection('[{"object": "Slab"}]')
            deleted = json.loads(ferrocad_shell.run_command("App_Delete"))
            """
        )
        cap.result("Delete response", deleted)
        cap.result("Objects", [obj.Name for obj in FreeCAD.ActiveDocument.Objects])
        cap.note(
            "Every call above is a plain function on the shell's Python module. The "
            "same functions are what the window calls while you click, which is why "
            "these pages run with no window at all."
        )

    with cap.step("Close the document"):
        cap.exec("FreeCAD.closeDocument('Toy')")
        cap.result("Open documents", sorted(FreeCAD.listDocuments()))


if __name__ == "__main__":
    main(
        run,
        slug=SLUG,
        quadrant=QUADRANT,
        title=TITLE,
        summary=SUMMARY,
        requires=REQUIRES,
    )
