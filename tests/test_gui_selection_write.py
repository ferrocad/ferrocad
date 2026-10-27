"""S4.3 write path: a script's selection reaches the shell.

The facade half (``addSelection``/``removeSelection``/``clearSelection`` queueing a
record) is covered by ``tests/test_gui_selection_object.py``. This file covers the
**shell bridge**: ``ferrocad_shell.take_selection_requests`` is the exact function
``ferrocad_gpui::python::take_selection_requests`` calls, and the record it returns is
the shape the Rust side deserialises into ``SelectionRequest``.

The shell-side application of those records (merge, remove, clear, re-arm) is covered
by ``crates/ferrocad_gpui/src/shell/tests/picking.rs``, where the real shell is
available.
"""

import json
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import FreeCAD
import FreeCADGui
import ferrocad_shell


class SelectionBridgeTest(unittest.TestCase):
    def setUp(self):
        if FreeCAD.getDocument("SelectionBridge") is not None:
            FreeCAD.closeDocument("SelectionBridge")
        self.doc = FreeCAD.newDocument("SelectionBridge")
        self.body = self.doc.addObject("App::FeaturePython", "Body")
        self.other = self.doc.addObject("App::FeaturePython", "Other")
        FreeCADGui.Selection._set([])
        FreeCADGui.Selection._take_requests()

    def tearDown(self):
        FreeCADGui.Selection._set([])
        FreeCADGui.Selection._take_requests()
        if FreeCAD.getDocument("SelectionBridge") is not None:
            FreeCAD.closeDocument("SelectionBridge")

    def drain(self):
        """The shell's entry point, decoded the way the bridge decodes it."""
        return json.loads(ferrocad_shell.take_selection_requests())

    def test_an_add_is_serialised_for_the_bridge(self):
        point = FreeCAD.Vector(4.0, 5.0, 6.0)
        self.assertTrue(FreeCADGui.Selection.addSelection("Body", "Face6", point))
        self.assertEqual(
            self.drain(),
            [
                {
                    "op": "add",
                    "object": "Body",
                    "elements": ["Face6"],
                    "points": [[4.0, 5.0, 6.0]],
                }
            ],
        )

    def test_the_queue_is_empty_after_one_drain(self):
        FreeCADGui.Selection.addSelection("Body")
        self.assertEqual(len(self.drain()), 1)
        self.assertEqual(self.drain(), [])

    def test_several_writes_keep_their_order(self):
        FreeCADGui.Selection.addSelection("Body", "Face6")
        FreeCADGui.Selection.addSelection("Other", "Edge1")
        FreeCADGui.Selection.removeSelection("Body", "Face6")
        FreeCADGui.Selection.clearSelection()
        self.assertEqual(
            [(record["op"], record["object"]) for record in self.drain()],
            [("add", "Body"), ("add", "Other"), ("remove", "Body"), ("clear", "")],
        )

    def test_a_preselection_is_a_record_too(self):
        FreeCADGui.Selection.setPreselection("Body", "Face2", FreeCAD.Vector(0, 0, 1))
        [record] = self.drain()
        self.assertEqual((record["op"], record["object"]), ("preselect", "Body"))
        self.assertEqual(record["elements"], ["Face2"])
        self.assertEqual(record["points"], [[0.0, 0.0, 1.0]])

    def test_a_shell_push_is_not_a_request(self):
        FreeCADGui.Selection._set([("Body", ["Face6"])])
        self.assertEqual(self.drain(), [])

    def test_a_point_of_any_vector_type_is_serialised(self):
        """A workbench may hand ``addSelection`` its own vector type."""

        class _Vec:
            def __init__(self, x, y, z):
                self.x, self.y, self.z = x, y, z

        FreeCADGui.Selection.addSelection("Body", "Face1", _Vec(1, 2, 3))
        [record] = self.drain()
        self.assertEqual(record["points"], [[1.0, 2.0, 3.0]])

    def test_an_unknown_object_queues_nothing(self):
        self.assertFalse(FreeCADGui.Selection.addSelection("Missing", "Face1"))
        self.assertEqual(self.drain(), [])

    def test_the_record_shape_matches_the_rust_deserialiser(self):
        """``SelectionRequest`` defaults every field but ``op``; none may be missing."""
        FreeCADGui.Selection.clearSelection()
        [record] = self.drain()
        self.assertEqual(
            set(record), {"op", "object", "elements", "points"}
        )
        self.assertIsInstance(record["op"], str)
        self.assertIsInstance(record["object"], str)
        self.assertIsInstance(record["elements"], list)
        self.assertIsInstance(record["points"], list)


if __name__ == "__main__":
    unittest.main()
