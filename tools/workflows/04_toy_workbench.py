"""User workflow: a workbench's four contracts, using the kernel-free toy edition.

Create an object, give it a visual representation, manipulate it through the same
generic interaction path the viewport's gizmos use, and record every edit as a
transaction. Nothing here needs a geometry kernel or a GPU.
"""

from _capture import main

import FreeCAD

QUADRANT = "how-to"
SLUG = "toy-workbench"
TITLE = "Create, display and manipulate an object"
SUMMARY = (
    "The toy workbench is the kernel-free reference edition. It shows the four "
    "things a workbench does: create an object, give it a visual representation, "
    "manipulate it through the generic interaction path, and record the edit as a "
    "transaction."
)
REQUIRES = [
    "the `ferrocad_toy` app script (it registers on import)",
    "no GPU: the mesh is read before it would be drawn (the snapshot is a "
    "committed image)",
]


def run(cap):
    with cap.step("Boot the edition"):
        cap.prose(
            "`bootstrap` builds the same sample document the shell opens at startup, "
            "then adds one slab and exposes the `Toy` console name. The shell calls "
            "exactly this function."
        )
        cap.exec(
            """
            import json
            import ferrocad_shell
            import ferrocad_toy

            payload = json.loads(ferrocad_toy.bootstrap("Toy"))
            """
        )
        cap.json("bootstrap", payload)

    with cap.step("The document the shell inspects"):
        cap.prose(
            "`model_tree` is the shell's inspector feed: documents, objects and their "
            "type names. It goes through the same public API a workbench uses."
        )
        cap.exec("tree = json.loads(ferrocad_shell.model_tree())")
        cap.json("model_tree", tree)

    with cap.step("The visual representation"):
        cap.prose(
            "The display provider turns the object's `Width`, `Depth`, `Height` and "
            "`Placement` into a mesh, and carries `Color` and `DisplayMode` alongside "
            "it. The shell asks for these meshes; it never names the workbench."
        )
        cap.exec("meshes = json.loads(ferrocad_shell.stage_meshes())")
        cap.exec(
            """
            slab_mesh = meshes[0]
            positions = slab_mesh["positions"]
            summary = {
                "object": slab_mesh["object"],
                "display": slab_mesh["display"],
                "color": slab_mesh["color"],
                "positions (vertices)": len(positions),
                "triangles": len(slab_mesh["triangles"]) // 3,
                "edges": len(slab_mesh["edges"]) // 2,
                "bounds": [
                    [min(p[i] for p in positions) for i in range(3)],
                    [max(p[i] for p in positions) for i in range(3)],
                ],
            }
            """
        )
        cap.json("stage_meshes[0], summarised", summary)

    with cap.step("Create an object"):
        cap.prose(
            "`makeSlab` is the workbench's creation command. It adds the object with "
            "its properties and opens one named transaction, so undo treats it as a "
            "single step."
        )
        cap.exec('slab = ferrocad_toy.makeSlab(width=60, height=30, x=10, name="Wide")')
        cap.result("Name", slab.Name)
        cap.result("Width", str(slab.Width))
        cap.result("Height", str(slab.Height))
        cap.result("Placement.Base", slab.Placement.Base)
        cap.result("DisplayMode", slab.DisplayMode)

    with cap.step("Manipulate it through the interaction path"):
        cap.prose(
            "A manipulator in the viewport resolves to a request, and the workbench "
            "supplies the edit. The `toy_height` grip writes `Height`; the generic "
            "`transform` gizmo composes a world-space matrix with the placement. Both "
            "arrive here as one transaction, so both are undoable."
        )
        cap.exec(
            """
            response = json.loads(
                ferrocad_shell.interaction(
                    "Wide", "toy_height", "set", "", json.dumps([42.0])
                )
            )
            """
        )
        cap.result("Response", response)
        cap.result("Height after the grip", str(slab.Height))
        cap.prose(
            "The transform gizmo sends a row-major 4×4 matrix; the translation lives "
            "in the last column (`m03`, `m13`, `m23`)."
        )
        cap.exec(
            """
            move = [1, 0, 0, 25, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]
            response = json.loads(
                ferrocad_shell.interaction(
                    "Wide", "transform", "apply", "", json.dumps(move)
                )
            )
            """
        )
        cap.result("Response", response)
        cap.result("Placement.Base after the move", slab.Placement.Base)

    with cap.step("What the manipulation looks like"):
        cap.prose(
            "The display provider answers with the mesh the viewport draws, built from "
            "`Width`, `Depth`, `Height` and `Placement` at the moment you ask. After the "
            "grip and the move that is 60 × 40 × 42 mm at x = 35 mm. The picture is "
            "that mesh, rendered offscreen by the same renderer the viewport uses."
        )
        cap.exec(
            """
            wide = next(
                mesh
                for mesh in json.loads(ferrocad_shell.stage_meshes())
                if mesh["object"] == "Wide"
            )
            wide_center = [
                (min(p[i] for p in wide["positions"]) + max(p[i] for p in wide["positions"]))
                / 2.0
                for i in range(3)
            ]
            wide_eye = [
                wide_center[0] - 100.0,
                wide_center[1] - 140.0,
                wide_center[2] + 95.0,
            ]
            """
        )
        cap.result("Mesh vertices", len(wide["positions"]))
        cap.result("Mesh triangles", len(wide["triangles"]) // 3)
        cap.image(
            "toy-slab.png",
            "The toy slab after the height grip and the move",
            caption=(
                "The `Wide` slab (60 × 40 × 42 mm at x = 35 mm), from "
                "`stage_meshes()`."
            ),
            mesh=wide,
            camera={
                "eye": wide_eye,
                "target": wide_center,
                "up": [0.0, 0.0, 1.0],
                "fov_y": 0.6,
            },
        )

    with cap.step("Undo the manipulation"):
        cap.prose(
            "Because each interaction is a transaction, the document's own undo stack "
            "reverses them one at a time. No workbench-side history is needed."
        )
        cap.exec(
            """
            slab.Document.undo()
            slab.Document.recompute()
            """
        )
        cap.result("Placement.Base after one undo (the move)", slab.Placement.Base)
        cap.result("Height after one undo", str(slab.Height))
        cap.exec(
            """
            slab.Document.undo()
            slab.Document.recompute()
            """
        )
        cap.result("Height after two undos", str(slab.Height))

    with cap.step("What the workbench registered"):
        cap.prose(
            "The commands in the list come from `FreeCADGui`, not from the shell. "
            "`enabled` is the command's `IsActive` evaluated against the current "
            "selection, which is why the edit commands are disabled right now."
        )
        cap.exec("commands = json.loads(ferrocad_shell.commands())")
        cap.table(
            ["Command", "Group", "Kind", "Enabled"],
            [
                [c["id"], c["group"], c["cmd_type"], c["enabled"]]
                for c in commands
                if c["group"] == "Toy" or c["cmd_type"] == "ForEdit"
            ],
        )
        cap.note(
            "The shell never links a workbench. Everything on this page reaches the "
            "document through `FreeCAD`, `FreeCADGui` or the module hook the edition "
            "registered at import time."
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
