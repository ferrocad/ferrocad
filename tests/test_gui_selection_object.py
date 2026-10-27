"""Tests for S4.3: ``Gui.Selection`` and its ``SelectionObject``.

Upstream's selection is a list of objects with ``Object``, ``SubElementNames``,
``PickedPoints`` and ``ObjectName``, and the controller both reads and writes it. The
shell owns the real viewport selection, so this facade:

* reads the snapshot the shell pushes (``_set``) and resolves it against the live
  document — a deleted object drops out;
* applies a write to its own snapshot so the writer observes it immediately, and
  queues the operation for the shell (``_take_requests``) instead of pretending it
  moved the viewport.

See ``docs/project/research/simplecad-bringup.md`` §5.5 and §7 S4.3.
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import FreeCAD
import FreeCADGui


class SelectionObjectTest(unittest.TestCase):
    def setUp(self):
        if FreeCAD.getDocument("SelectionEx") is not None:
            FreeCAD.closeDocument("SelectionEx")
        self.doc = FreeCAD.newDocument("SelectionEx")
        self.a = self.doc.addObject("App::FeaturePython", "A")
        self.b = self.doc.addObject("App::FeaturePython", "B")
        FreeCADGui.Selection._set([])
        FreeCADGui.Selection._take_requests()

    def tearDown(self):
        FreeCADGui.Selection._set([])
        FreeCADGui.Selection._take_requests()
        if FreeCAD.getDocument("SelectionEx") is not None:
            FreeCAD.closeDocument("SelectionEx")

    def test_the_snapshot_exposes_the_upstream_members(self):
        FreeCADGui.Selection._set([("A", []), ("B", ["Face6", "Edge3"])])
        items = FreeCADGui.Selection.getSelectionEx()
        self.assertEqual([item.ObjectName for item in items], ["A", "B"])
        self.assertEqual([item.Object.Name for item in items], ["A", "B"])
        self.assertEqual(items[0].SubElementNames, [])
        self.assertEqual(items[1].SubElementNames, ["Face6", "Edge3"])
        self.assertEqual(items[1].PickedPoints, [])
        self.assertFalse(items[0].HasSubObjects)
        self.assertTrue(items[1].HasSubObjects)

    def test_a_selection_object_still_unpacks_as_a_tuple(self):
        FreeCADGui.Selection._set([("B", ["Face6"])])
        [(obj, elements)] = FreeCADGui.Selection.getSelectionEx()
        self.assertIs(obj, self.b)
        self.assertEqual(elements, ["Face6"])
        first, second = FreeCADGui.Selection.getSelectionEx()[0]
        self.assertIs(first, self.b)
        self.assertEqual(second, ["Face6"])

    def test_get_selection_ex_can_filter_by_object_name(self):
        FreeCADGui.Selection._set([("A", []), ("B", ["Face6"])])
        items = FreeCADGui.Selection.getSelectionEx("B")
        self.assertEqual([item.ObjectName for item in items], ["B"])
        self.assertEqual(FreeCADGui.Selection.getSelectionEx("Missing"), [])

    def test_the_shell_push_can_carry_picked_points(self):
        point = FreeCAD.Vector(1.0, 2.0, 3.0)
        FreeCADGui.Selection._set([("A", ["Face1"], [point])])
        [item] = FreeCADGui.Selection.getSelectionEx()
        self.assertEqual(item.PickedPoints, [point])
        self.assertEqual(FreeCADGui.Selection._take_requests(), [])

    def test_a_deleted_object_drops_out_of_the_snapshot(self):
        FreeCADGui.Selection._set([("A", []), ("B", [])])
        self.doc.removeObject("A")
        self.assertEqual([item.ObjectName for item in FreeCADGui.Selection.getSelectionEx()], ["B"])

    def test_is_selected_accepts_an_object_a_name_and_a_sub_element(self):
        FreeCADGui.Selection._set([("B", ["Face6"])])
        self.assertTrue(FreeCADGui.Selection.isSelected(self.b))
        self.assertTrue(FreeCADGui.Selection.isSelected("B"))
        self.assertTrue(FreeCADGui.Selection.isSelected("B", "Face6"))
        self.assertFalse(FreeCADGui.Selection.isSelected("B", "Face1"))
        self.assertFalse(FreeCADGui.Selection.isSelected("A"))

    def test_a_whole_object_selection_covers_every_sub_element(self):
        FreeCADGui.Selection._set([("B", [])])
        self.assertTrue(FreeCADGui.Selection.isSelected("B", "Face6"))

    def test_add_selection_records_the_point_and_queues_the_write(self):
        point = FreeCAD.Vector(4.0, 5.0, 6.0)
        self.assertTrue(FreeCADGui.Selection.addSelection("B", "Face6", point))
        [item] = FreeCADGui.Selection.getSelectionEx()
        self.assertIs(item.Object, self.b)
        self.assertEqual(item.SubElementNames, ["Face6"])
        self.assertEqual(item.PickedPoints, [point])
        self.assertEqual(
            FreeCADGui.Selection._take_requests(),
            [
                {
                    "op": "add",
                    "object": "B",
                    "elements": ["Face6"],
                    "points": [point],
                }
            ],
        )

    def test_add_selection_merges_sub_elements_and_accepts_a_list(self):
        self.assertTrue(FreeCADGui.Selection.addSelection(self.b, ["Face1", "Face2"]))
        self.assertTrue(FreeCADGui.Selection.addSelection("B", "Face2"))
        [item] = FreeCADGui.Selection.getSelectionEx()
        self.assertEqual(item.SubElementNames, ["Face1", "Face2"])

    def test_a_whole_object_add_wins_over_a_stale_sub_element_list(self):
        FreeCADGui.Selection.addSelection("B", "Face6")
        self.assertTrue(FreeCADGui.Selection.addSelection("B"))
        [item] = FreeCADGui.Selection.getSelectionEx()
        self.assertEqual(item.SubElementNames, [])

    def test_an_unknown_object_is_not_invented(self):
        self.assertFalse(FreeCADGui.Selection.addSelection("Missing", "Face1"))
        self.assertEqual(FreeCADGui.Selection.getSelection(), [])
        self.assertEqual(FreeCADGui.Selection._take_requests(), [])

    def test_remove_selection_drops_one_sub_element_or_the_whole_object(self):
        FreeCADGui.Selection._set([("B", ["Face6", "Edge3"])])
        self.assertTrue(FreeCADGui.Selection.removeSelection("B", "Face6"))
        [item] = FreeCADGui.Selection.getSelectionEx()
        self.assertEqual(item.SubElementNames, ["Edge3"])
        self.assertFalse(FreeCADGui.Selection.removeSelection("B", "Face1"))
        self.assertTrue(FreeCADGui.Selection.removeSelection("B"))
        self.assertEqual(FreeCADGui.Selection.getSelection(), [])
        operations = [request["op"] for request in FreeCADGui.Selection._take_requests()]
        self.assertEqual(operations, ["remove", "remove"])

    def test_clear_selection_empties_the_snapshot_and_queues_it(self):
        FreeCADGui.Selection._set([("A", []), ("B", ["Face6"])])
        FreeCADGui.Selection._take_requests()
        FreeCADGui.Selection.clearSelection()
        self.assertEqual(FreeCADGui.Selection.getSelection(), [])
        self.assertEqual([request["op"] for request in FreeCADGui.Selection._take_requests()], ["clear"])

    def test_clear_selection_can_be_limited_to_one_document(self):
        FreeCADGui.Selection._set([("A", []), ("B", [])])
        FreeCADGui.Selection.clearSelection("SelectionEx")
        self.assertEqual(FreeCADGui.Selection.getSelection(), [])
        FreeCADGui.Selection._take_requests()

    def test_preselection_round_trips_as_a_selection_object(self):
        self.assertIsNone(FreeCADGui.Selection.getPreselection())
        point = FreeCAD.Vector(0.0, 0.0, 7.0)
        FreeCADGui.Selection.setPreselection(self.a, "Face3", point)
        item = FreeCADGui.Selection.getPreselection()
        self.assertIs(item.Object, self.a)
        self.assertEqual(item.SubElementNames, ["Face3"])
        self.assertEqual(item.PickedPoints, [point])
        request = FreeCADGui.Selection._take_requests()[0]
        self.assertEqual((request["op"], request["object"]), ("preselect", "A"))

    def test_a_preselection_of_a_missing_object_reports_none(self):
        FreeCADGui.Selection.setPreselection("Missing")
        self.assertIsNone(FreeCADGui.Selection.getPreselection())

    def test_the_shell_push_clears_the_preselection(self):
        FreeCADGui.Selection.setPreselection("A", "Face1")
        FreeCADGui.Selection._set([("B", [])])
        self.assertIsNone(FreeCADGui.Selection.getPreselection())


if __name__ == "__main__":
    unittest.main()
