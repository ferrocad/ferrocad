"""Tutorial: build one small parametric model from an empty interpreter.

This is the lesson. It is deliberately linear — no branches, no error handling —
so a first-time reader can run it top to bottom and end with a saved document.
The how-to guides take each of these ideas further.
"""

import FreeCAD

from _capture import main

QUADRANT = "tutorials"
SLUG = "first-model"
TITLE = "Your first model"
SUMMARY = (
    "A single pass from an empty interpreter to a saved, parametric document: a "
    "document, a parameter, a second object that depends on it, a recompute, and the "
    "save that keeps it."
)
REQUIRES = ["the `FreeCAD` facade; no workbench module and no GPU"]


def run(cap):
    with cap.step("Import the application"):
        cap.prose(
            "Everything starts with the `FreeCAD` module. In FerroCAD it is a "
            "Rust-backed implementation of the same module upstream FreeCAD ships, so "
            "the code in this tutorial is ordinary FreeCAD Python."
        )
        cap.exec("import FreeCAD")
        cap.result("Version", ".".join(FreeCAD.Version()[:3]))
        cap.result("Backend", FreeCAD.backend)

    with cap.step("Make a document"):
        cap.prose(
            "A document is the file you are building. `newDocument` creates it, makes "
            "it active, and returns it. Nothing is on disk yet."
        )
        cap.exec("doc = FreeCAD.newDocument('FirstModel')")
        cap.result("Active document", FreeCAD.ActiveDocument.Name)

    with cap.step("Add a parameter"):
        cap.prose(
            "Objects are created by type name. `App::FeaturePython` is the generic "
            "scripted object; properties are added afterwards, each with a group and a "
            "description so a property editor has something to show."
        )
        cap.exec(
            """
            parameters = doc.addObject("App::FeaturePython", "Parameters")
            parameters.addProperty(
                "App::PropertyLength", "Length", "Model", "The one number that drives it"
            )
            parameters.Length = "20 mm"
            """
        )
        cap.result("Parameters.Length", str(parameters.Length))

    with cap.step("Make something depend on it"):
        cap.prose(
            "A second object can read the first one through an expression. The "
            "expression is stored in the document, which is what makes the model "
            "parametric: the value follows the input on every recompute."
        )
        cap.exec(
            """
            result = doc.addObject("App::FeaturePython", "Result")
            result.addProperty("App::PropertyLength", "Width", "Model", "Derived width")
            result.setExpression("Width", "Parameters.Length * 3")
            doc.recompute()
            """
        )
        cap.result("Result.Width", str(result.Width))
        cap.result("Result.State", result.State)

    with cap.step("Change the input"):
        cap.prose(
            "Editing the parameter marks the dependent object for recompute. One call "
            "updates everything in dependency order — you never tell it what to update."
        )
        cap.exec(
            """
            parameters.Length = "35 mm"
            doc.recompute()
            """
        )
        cap.result("Parameters.Length", str(parameters.Length))
        cap.result("Result.Width", str(result.Width))

    with cap.step("Save it"):
        cap.prose(
            "`saveAs` writes the document, including the expression. The path below is "
            "a temporary one; in your own script choose a real location. Opening it "
            "again gives back the same objects and the same values."
        )
        cap.exec(
            """
            import os, tempfile
            path = os.path.join(tempfile.mkdtemp(prefix="ferrocad-tutorial-"), "first.FCStd")
            doc.saveAs(path)
            """
        )
        cap.exec(
            """
            FreeCAD.closeDocument("FirstModel")
            reopened = FreeCAD.open(path)
            """
        )
        cap.result("Objects after reopening", [o.Name for o in reopened.Objects])
        cap.result(
            "Width after reopening", str(reopened.getObject("Result").Width)
        )

    with cap.step("Where to go next"):
        cap.prose(
            "That is the whole loop: create, parameterise, recompute, save. The how-to "
            "guides take each part further — properties and validation, undo and "
            "persistence in detail, and the workbench contracts that put objects on "
            "screen."
        )
        cap.exec(
            """
            import shutil
            FreeCAD.closeDocument(reopened.Name)
            shutil.rmtree(os.path.dirname(path))
            """
        )
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
