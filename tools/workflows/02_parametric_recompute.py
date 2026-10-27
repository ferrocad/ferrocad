"""User workflow: a dependency between two objects, driven by an expression."""

from _capture import main

import FreeCAD

QUADRANT = "how-to"
SLUG = "parametric-recompute"
TITLE = "Wire two objects together with an expression"
SUMMARY = (
    "Add an expression that reads one object's length from another, recompute, then "
    "change the input and watch the dependent value follow."
)
REQUIRES = ["the `FreeCAD` facade; no workbench module and no GPU"]


def run(cap):
    with cap.step("Two objects, one dependency"):
        cap.prose(
            "Expressions are evaluated on recompute, not on assignment. The document "
            "records the dependency edge, so it can order the objects without the "
            "script doing so."
        )
        cap.exec(
            """
            doc = FreeCAD.newDocument("Parametric")

            source = doc.addObject("App::FeaturePython", "Source")
            source.addProperty("App::PropertyLength", "Length", "Base", "Input length")
            source.Length = "10 mm"

            derived = doc.addObject("App::FeaturePython", "Derived")
            derived.addProperty("App::PropertyLength", "Result", "Base", "Computed")
            derived.setExpression("Result", "Source.Length * 2")
            """
        )
        cap.exec("doc.recompute()")
        cap.result("Source.Length", str(source.Length))
        cap.result("Derived.Result", str(derived.Result))
        cap.result("Derived.State", derived.State)

    with cap.step("Change the input"):
        cap.prose(
            "Assigning to the source does not touch `Derived`; the next recompute "
            "does, because the dependency is stored in the document."
        )
        cap.exec(
            """
            source.Length = "15 mm"
            doc.recompute()
            """
        )
        cap.result("Source.Length", str(source.Length))
        cap.result("Derived.Result", str(derived.Result))

    with cap.step("What the result describes"):
        cap.prose(
            "`Derived.Result` is 30 mm after the input changed. The box below uses "
            "that computed value; the depth and height are illustrative."
        )
        cap.exec(
            """
            box_size = (float(derived.Result), 20.0, 10.0)
            box_center = [value / 2.0 for value in box_size]
            span = max(box_size)
            box_eye = [
                box_center[0] - span * 0.8,
                box_center[1] - span * 1.8,
                box_center[2] + span * 1.5,
            ]
            """
        )
        cap.result("Box (result, depth, height)", box_size)
        cap.image(
            "parametric-recompute.png",
            "A 30 by 20 by 10 millimetre box drawn in the viewport",
            caption=(
                "The 30 mm `Derived.Result` after the input changed to 15 mm, drawn "
                "as a box; the depth and height are illustrative."
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

    with cap.step("Read the expression back"):
        cap.prose("The expression is part of the document, not of the Python object.")
        cap.exec("expr = derived.ExpressionEngine")
        cap.json("ExpressionEngine", [list(item) for item in expr])

    with cap.step("Close the document"):
        cap.exec("FreeCAD.closeDocument('Parametric')")
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
