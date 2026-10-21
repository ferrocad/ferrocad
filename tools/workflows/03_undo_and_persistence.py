"""User workflow: transactions, undo/redo, and a save/reload round trip."""

from _capture import main

import FreeCAD

SLUG = "undo-and-persistence"
TITLE = "Undo an edit and reload the document"
SUMMARY = (
    "Group an edit into a named transaction, undo and redo it, then save the "
    "document, close it, open it again, and read the values back."
)
REQUIRES = ["the `FreeCAD` facade; a writable temporary directory"]


def run(cap):
    with cap.step("Build a document"):
        cap.exec("import tempfile")
        cap.exec(
            """
            doc = FreeCAD.newDocument("Undoable")

            part = doc.addObject("App::FeaturePython", "Part")
            part.addProperty("App::PropertyLength", "Length", "Base", "Overall length")
            part.addProperty("App::PropertyEnumeration", "Finish", "Base", "Surface finish")
            part.Finish = ["As machined", "Anodised"]
            part.Length = "10 mm"
            doc.recompute()
            """
        )
        cap.result("part.Length", str(part.Length))

    with cap.step("One transaction, one undo"):
        cap.prose(
            "Everything between `openTransaction` and `commitTransaction` is one "
            "undoable step. A workbench wraps each command in exactly this, which is "
            "why undo never lands in the middle of a command."
        )
        cap.exec(
            """
            doc.openTransaction("Resize")
            part.Length = "25 mm"
            part.Finish = "Anodised"
            doc.commitTransaction()
            doc.recompute()
            """
        )
        cap.result("part.Length after the edit", str(part.Length))
        cap.result("part.Finish after the edit", part.Finish)
        cap.exec("doc.undo(); doc.recompute()")
        cap.result("part.Length after undo", str(part.Length))
        cap.result("part.Finish after undo", part.Finish)
        cap.exec("doc.redo(); doc.recompute()")
        cap.result("part.Length after redo", str(part.Length))

    with cap.step("Save and reload"):
        cap.prose(
            "`saveAs` writes the document format; `open` reads it into a new document "
            "with the same names and property values. The path is a temporary file, so "
            "it is not shown."
        )
        cap.exec(
            """
            import os
            path = os.path.join(tempfile.mkdtemp(prefix="ferrocad-userdocs-"), "undoable.FCStd")
            doc.saveAs(path)
            doc_name = doc.Name
            """
        )
        cap.exec("FreeCAD.closeDocument(doc_name)")
        cap.result("Open documents after close", sorted(FreeCAD.listDocuments()))
        cap.exec("reopened = FreeCAD.open(path)")
        cap.exec(
            """
            reloaded = reopened.getObject("Part")
            """
        )
        cap.result("Reloaded name", reloaded.Label)
        cap.result("Reloaded Length", str(reloaded.Length))
        cap.result("Reloaded Finish", reloaded.Finish)

    with cap.step("Clean up"):
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
        title=TITLE,
        summary=SUMMARY,
        requires=REQUIRES,
    )
