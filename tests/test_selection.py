"""Tests for the ``FreeCADGui.Selection`` snapshot and the built-in commands (A2).

The shell owns the selection and pushes it to the facade; these tests drive the
snapshot directly (the shell's own push is covered by
``crates/ferrocad_toy/tests/selection.rs``). See ``docs/workbenches/actions-and-tools.md`` §5
and §9.
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import FreeCAD
import FreeCADGui

import ferrocad_shell  # noqa: F401  (registers App_Delete / App_Move)


class SelectionTests(unittest.TestCase):
    def setUp(self):
        if FreeCAD.getDocument("Selection") is not None:
            FreeCAD.closeDocument("Selection")
        self.doc = FreeCAD.newDocument("Selection")
        self.a = self.doc.addObject("App::FeaturePython", "A")
        self.b = self.doc.addObject("App::FeaturePython", "B")
        for obj in (self.a, self.b):
            obj.addProperty("App::PropertyPlacement", "Placement", "Base", "Placement")
        FreeCADGui.Selection._set([])

    def tearDown(self):
        FreeCADGui.Selection._set([])
        if FreeCAD.getDocument("Selection") is not None:
            FreeCAD.closeDocument("Selection")

    def entry(self, name):
        return next(c for c in FreeCADGui._command_list() if c["id"] == name)

    def test_the_snapshot_resolves_objects_and_subelements(self):
        FreeCADGui.Selection._set([("A", []), ("B", ["Face6"])])
        self.assertEqual(
            [obj.Name for obj in FreeCADGui.Selection.getSelection()], ["A", "B"]
        )
        self.assertEqual(
            [(obj.Name, elements) for obj, elements in FreeCADGui.Selection.getSelectionEx()],
            [("A", []), ("B", ["Face6"])],
        )
        self.assertTrue(FreeCADGui.Selection.isSelected("A"))
        self.assertTrue(FreeCADGui.Selection.isSelected(self.b))
        self.assertFalse(FreeCADGui.Selection.isSelected("Missing"))
        self.assertFalse(FreeCADGui.Selection.isSelected("C"))

    def test_a_deleted_name_drops_out_of_the_snapshot(self):
        FreeCADGui.Selection._set([("A", []), ("B", [])])
        self.doc.removeObject("A")
        self.assertEqual(
            [obj.Name for obj in FreeCADGui.Selection.getSelection()], ["B"]
        )

    def test_is_active_follows_the_snapshot(self):
        self.assertFalse(self.entry("App_Delete")["enabled"])
        self.assertFalse(self.entry("App_Move")["enabled"])
        FreeCADGui.Selection._set([("A", [])])
        self.assertTrue(self.entry("App_Delete")["enabled"])
        self.assertTrue(self.entry("App_Move")["enabled"])
        FreeCADGui.Selection._set([])
        self.assertFalse(self.entry("App_Delete")["enabled"])

    def test_delete_removes_the_selection_in_one_undo_step(self):
        FreeCADGui.Selection._set([("A", []), ("B", [])])
        result = FreeCADGui._run("App_Delete")
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["status"], "deleted 2 object(s)")
        self.assertEqual(self.doc.CountObjects, 0)

        self.assertTrue(self.doc.undo())
        self.assertEqual(sorted(obj.Name for obj in self.doc.Objects), ["A", "B"])

    def test_delete_with_nothing_selected_is_harmless(self):
        result = FreeCADGui._run("App_Delete")
        self.assertTrue(result["ok"])
        self.assertEqual(result["status"], "nothing selected")
        self.assertEqual(self.doc.CountObjects, 2)

    def test_move_translates_every_selected_object(self):
        FreeCADGui.Selection._set([("A", []), ("B", [])])
        result = FreeCADGui.runCommand("App_Move", 10.0, 0.0, 0.0)
        self.assertEqual(result["status"], "moved 2 object(s)")
        for obj in (self.a, self.b):
            self.assertAlmostEqual(obj.Placement.Base.x, 10.0, places=6)

        self.assertTrue(self.doc.undo())
        for obj in (self.a, self.b):
            self.assertAlmostEqual(obj.Placement.Base.x, 0.0, places=6)

    def test_a_bare_move_opens_its_dialog_and_cancel_keeps_the_object(self):
        FreeCADGui.Selection._set([("A", [])])
        result = FreeCADGui._run("App_Move")
        self.assertTrue(result["ok"])
        self.assertEqual(result["dialog"]["title"], "Move")
        self.assertEqual(len(result["dialog"]["fields"]), 3)
        token = result["token"]

        self.assertTrue(FreeCADGui._cancel(token)["ok"])
        self.assertFalse(FreeCADGui._accept(token, {})["ok"], "the token is spent")
        self.assertAlmostEqual(self.a.Placement.Base.x, 0.0, places=6)


if __name__ == "__main__":
    unittest.main()
