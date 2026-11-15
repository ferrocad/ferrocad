"""The parametric edition's sketch tools: `Sketch_Polyline`, `Sketch_Dimension`, drag.

Three surfaces are under test, all through the same entry points the shell uses:

* the **commands** — `Sketch_Polyline` arms the sketch tool and `Sketch_Dimension`
  adds a driving datum, while `Part_Polyline` keeps its own tool id;
* the **creating commit** — the points a finished polyline tool hands over become a
  constrained `Sketcher::SketchObject` plus the `Part::Extrusion` that links it, in one
  undo step;
* the **vertex grips** — the edition's manipulator provider declares one `plane` handle
  per sketch vertex, and its `sketch_drag` commit moves the point through
  `SketchObject.drag`, so the constraints decide where it lands.

The Rust half of the grip (the plane drag maths and the JSON transport) is pinned in
`crates/ferrocad_stage/src/interaction/declarative.rs`,
`crates/ferrocad_shell_core/src/workbench/manipulators.rs` and
`crates/ferrocad_parametric/tests/sketch_drag_round_trip.rs`.

Run::

    python3 tools/run_tests.py --extension workbenches
"""

from __future__ import annotations

import json
import unittest

import FreeCAD
import FreeCADGui
import Part  # noqa: F401  (links the kernel)
import Sketcher

import ferrocad_parametric  # noqa: F401  (registers the commands and providers)
import ferrocad_shell

#: Four points of the profile on the plane `x = 5`: local `(u, v)` = world `(y, z)`.
SQUARE = [5.0, 0.0, 0.0, 5.0, 10.0, 0.0, 5.0, 10.0, 10.0, 5.0, 0.0, 10.0]


def _commit(manipulator, action, element, values, obj):
    """Run the shell's interaction entry and decode its answer."""
    raw = ferrocad_shell.interaction(
        obj, manipulator, action, element, json.dumps(values)
    )
    return json.loads(raw)


def _specs(obj, element=None):
    raw = ferrocad_shell.manipulator_specs(json.dumps({"object": obj, "element": element}))
    return json.loads(raw)["specs"]


class SketchToolsCase(unittest.TestCase):
    def setUp(self):
        self.doc = FreeCAD.newDocument(self.id().replace(".", "_"))

    def tearDown(self):
        if FreeCAD.getDocument(self.doc.Name) is not None:
            FreeCAD.closeDocument(self.doc.Name)

    def _draw(self):
        """Create the sketch and its pad through the creating commit."""
        answer = _commit("sketch_polyline", "create", "", SQUARE, "")
        self.assertTrue(answer["ok"], answer)
        return self.doc.getObject("Sketch"), self.doc.getObject("Sketch_Extrusion")

    def test_the_sketch_commands_are_distinct_from_the_wire_tool(self):
        self.assertEqual(
            FreeCADGui.runCommand("Sketch_Polyline"), {"tool": "sketch_polyline"}
        )
        self.assertEqual(FreeCADGui.runCommand("Part_Polyline"), {"tool": "polyline"})

    def test_the_creating_commit_builds_a_constrained_sketch_and_its_pad(self):
        sketch, extrusion = self._draw()
        self.assertIsNotNone(sketch)
        self.assertIsNotNone(extrusion)
        self.assertEqual(sketch.TypeId, "Sketcher::SketchObject")
        self.assertEqual(sketch.GeometryCount, 4)
        # Four shared corners, the origin pair staged for both segments meeting there,
        # and the two horizontal + two vertical autoconstraints.
        self.assertEqual(sketch.ConstraintCount, 10)
        self.assertEqual(sketch.solve(), 2)
        self.assertEqual(sketch.RedundantConstraints, [6])
        self.assertFalse(sketch.Visibility, "the sketch is hidden behind its pad")

        box = extrusion.Shape.BoundBox
        self.assertAlmostEqual(box.XMin, 5.0, places=6)
        self.assertAlmostEqual(box.XMax, 15.0, places=6)
        self.assertAlmostEqual(box.YLength, 10.0, places=6)
        self.assertAlmostEqual(box.ZLength, 10.0, places=6)

        # One undo takes the sketch and the pad together, because they are one commit.
        # (`App_Undo` is the real path: the document restores the inputs, and the app
        # recomputes so the derived `Shape` follows.)
        self.assertTrue(FreeCADGui.runCommand("App_Undo")["ok"])
        self.assertIsNone(self.doc.getObject("Sketch"))
        self.assertIsNone(self.doc.getObject("Sketch_Extrusion"))

    def test_a_dimension_is_a_driving_datum_at_the_edge_value(self):
        sketch, extrusion = self._draw()
        FreeCADGui.Selection.clearSelection()
        FreeCADGui.Selection.addSelection(sketch, "Edge1")
        answer = FreeCADGui.runCommand("Sketch_Dimension")
        self.assertIn("Distance", answer["status"])
        self.assertEqual(sketch.ConstraintCount, 11)
        self.assertEqual(sketch.solve(), 1, "one datum leaves one free parameter")

        # The datum is the one the command named, and editing it resizes the profile.
        sketch.setDatum(10, 20.0)
        self.doc.recompute()
        self.assertAlmostEqual(extrusion.Shape.BoundBox.YLength, 20.0, places=6)

    def test_the_provider_declares_one_plane_grip_per_vertex(self):
        sketch, _ = self._draw()
        specs = _specs("Sketch")
        self.assertEqual(len(specs), 4, "one grip per shape vertex")

        names = [spec["element"] for spec in specs]
        self.assertEqual(names, ["Vertex1", "Vertex2", "Vertex3", "Vertex4"])
        for spec in specs:
            self.assertEqual(spec["id"], "sketch_drag")
            self.assertEqual(spec["action"], "drag")
            self.assertEqual(spec["kind"], "plane", "a vertex drags in its plane")
            self.assertEqual(spec["value"], 0.0)
            self.assertEqual(spec["value_v"], 0.0)
            # The frame is the sketch's own: u = world +Y, v = world +Z, normal = +X,
            # and the origin is the vertex itself.
            self.assertAlmostEqual(spec["frame"]["x"][1], 1.0, places=9)
            self.assertAlmostEqual(spec["frame"]["y"][2], 1.0, places=9)
            self.assertAlmostEqual(spec["frame"]["z"][0], 1.0, places=9)
            self.assertAlmostEqual(spec["frame"]["origin"][0], 5.0, places=9)

        # A named vertex narrows the set to that grip.
        narrowed = _specs("Sketch", "Vertex2")
        self.assertEqual([spec["element"] for spec in narrowed], ["Vertex2"])
        self.assertAlmostEqual(narrowed[0]["frame"]["origin"][1], 10.0, places=9)

        # A non-sketch object declares nothing here.
        self.assertEqual(_specs("Sketch_Extrusion"), [])

    def test_the_drag_commit_keeps_the_constraints_and_moves_the_pad(self):
        sketch, extrusion = self._draw()
        # Vertex2 is segment 0's end. `Horizontal` fixes its height and nothing fixes the
        # width, so the u component takes and the v component is refused.
        answer = _commit("sketch_drag", "drag", "Vertex2", [5.0, 3.0], "Sketch")
        self.assertTrue(answer["ok"], answer)

        self.assertEqual(sketch.solve(), 2, "a drag leaves the free DoF alone")
        self.assertEqual(sketch.RedundantConstraints, [6])
        box = extrusion.Shape.BoundBox
        self.assertAlmostEqual(box.YLength, 15.0, places=6)
        self.assertAlmostEqual(box.ZLength, 10.0, places=6)

        # The whole drag is one undo step, and it restores the profile and the pad.
        self.assertTrue(FreeCADGui.runCommand("App_Undo")["ok"])
        self.assertAlmostEqual(
            self.doc.getObject("Sketch_Extrusion").Shape.BoundBox.YLength, 10.0, places=6
        )

    def test_a_drag_on_a_vertex_that_does_not_exist_is_refused(self):
        self._draw()
        answer = _commit("sketch_drag", "drag", "Vertex99", [1.0, 1.0], "Sketch")
        self.assertFalse(answer["ok"])
        self.assertIn("not a point", answer["error"])

    def test_a_drag_needs_both_coordinates(self):
        self._draw()
        answer = _commit("sketch_drag", "drag", "Vertex2", [1.0], "Sketch")
        self.assertFalse(answer["ok"])
        self.assertIn("two coordinates", answer["error"])


if __name__ == "__main__":
    unittest.main()
