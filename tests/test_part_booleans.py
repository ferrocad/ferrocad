"""The Part boolean and compound commands.

`Part_Fuse`, `Part_Cut`, `Part_Common` and `Part_Compound` are the selection-driven
half of the container objects: they police the selection, build one object in one
undo step, and leave the geometry to `Part::MultiFuse` / `Part::Cut` /
`Part::MultiCommon` / `Part::Compound`. These tests pin the command contract — the
selection rules, the resulting property wiring, the hidden operands, the
single-step undo — and the shapes those objects produce.

The shape-side facts P2 relies on are pinned here too, because they are what a user
sees after the command:

* `Refine` defaults on and unifies the seam two touching bodies leave (ten faces
  become six), and turning it off brings the seam back;
* a lone compound argument is flattened, so `Part_Fuse` over one compound unions its
  children instead of folding one shape;
* `MultiCommon.Behavior` chooses between "every shape" and "first against the rest".

`Part` and the parametric script are required; run with
``tools/run_tests.py --extension workbenches``, not `unittest discover` in a
base-only checkout.
"""

from __future__ import annotations

import json
import unittest

import FreeCAD
import FreeCADGui
import ferrocad_parametric  # noqa: F401  (registers the commands)
import ferrocad_shell
import Part
from FreeCAD import Vector


def _box(name, x=0.0, y=0.0, z=0.0, size=10.0):
    """A `Part::Feature` box whose stored shape carries its placement."""
    doc = FreeCAD.ActiveDocument
    obj = doc.addObject("Part::Feature", name)
    obj.Shape = Part.makeBox(size, size, size, Vector(x, y, z))
    ferrocad_parametric._add_appearance(obj)
    return obj


def _enabled(command_id):
    """Whether the shell would enable `command_id` (its `IsActive` answer)."""
    for entry in json.loads(ferrocad_shell.commands()):
        if entry["id"] == command_id:
            return entry["enabled"]
    raise AssertionError(f"{command_id} is not registered")


def _volume(obj):
    return obj.Shape.Volume


class BooleanCommandTests(unittest.TestCase):
    def setUp(self):
        self.doc = FreeCAD.newDocument("BooleanCommandTests")

    def tearDown(self):
        if FreeCAD.getDocument(self.doc.Name) is not None:
            FreeCAD.closeDocument(self.doc.Name)

    def select(self, *objects):
        FreeCADGui.Selection.clearSelection()
        for obj in objects:
            FreeCADGui.Selection.addSelection(obj)

    def test_union_wires_a_multi_fuse_over_the_selection(self):
        a = _box("BoxA")
        b = _box("BoxB", x=5.0)
        self.select(a, b)

        status = FreeCADGui.runCommand("Part_Fuse", 0)
        self.assertIsNotNone(status)

        fusion = self.doc.getObject("Fusion")
        self.assertIsNotNone(fusion, "the command created a Part::MultiFuse")
        self.assertEqual(fusion.TypeId, "Part::MultiFuse")
        self.assertEqual([link.Name for link in fusion.Shapes], ["BoxA", "BoxB"])
        # The overlap is counted once: 2 × 1000 − 500.
        self.assertAlmostEqual(_volume(fusion), 1500.0, places=6)
        self.assertEqual(len(fusion.Shape.Solids), 1, "one body")
        self.assertTrue(fusion.Shape.isValid())

        self.assertFalse(a.Visibility, "the operands are hidden")
        self.assertFalse(b.Visibility)

    def test_cut_needs_exactly_two_shapes(self):
        a, b, c = _box("BoxA"), _box("BoxB", x=5.0), _box("BoxC", y=5.0)
        self.select(a, b, c)
        self.assertFalse(
            _enabled("Part_Cut"),
            "three shapes is not a Cut selection",
        )
        FreeCADGui.runCommand("Part_Cut", 0)
        self.assertIsNone(self.doc.getObject("Cut"))

        self.select(a, b)
        self.assertTrue(_enabled("Part_Cut"))
        FreeCADGui.runCommand("Part_Cut", 0)
        cut = self.doc.getObject("Cut")
        self.assertIsNotNone(cut)
        self.assertEqual(cut.TypeId, "Part::Cut")
        # Base is the first selected object, Tool the second.
        self.assertEqual(cut.Base.Name, "BoxA")
        self.assertEqual(cut.Tool.Name, "BoxB")
        # BoxA − BoxB is the 10 × 10 × 10 box minus the 5 × 10 × 10 overlap.
        self.assertAlmostEqual(_volume(cut), 500.0, places=6)

    def test_intersection_uses_the_common_of_every_shape(self):
        a = _box("BoxA")
        b = _box("BoxB", x=5.0)
        self.select(a, b)
        FreeCADGui.runCommand("Part_Common", 0)

        common = self.doc.getObject("Common")
        self.assertIsNotNone(common)
        self.assertEqual(common.TypeId, "Part::MultiCommon")
        self.assertAlmostEqual(_volume(common), 500.0, places=6)

    def test_a_refused_selection_creates_nothing(self):
        a = _box("BoxA")
        self.select(a)
        self.assertFalse(_enabled("Part_Fuse"))
        FreeCADGui.runCommand("Part_Fuse", 0)
        self.assertIsNone(self.doc.getObject("Fusion"))

    def test_the_command_is_one_undo_step(self):
        a = _box("BoxA")
        b = _box("BoxB", x=5.0)
        self.select(a, b)
        undo = self.doc.UndoCount
        FreeCADGui.runCommand("Part_Fuse", 0)
        self.assertEqual(
            self.doc.UndoCount, undo + 1, "one transaction, not one per object"
        )
        self.doc.undo()
        self.assertIsNone(self.doc.getObject("Fusion"), "undo removes the fusion")
        self.assertIsNotNone(self.doc.getObject("BoxA"), "the operands survive")

    def test_a_lone_compound_is_fused_through_its_children(self):
        a = _box("BoxA")
        b = _box("BoxB", x=5.0)
        self.select(a, b)
        FreeCADGui.runCommand("Part_Compound", 0)
        compound = self.doc.getObject("Compound")
        self.assertIsNotNone(compound)
        self.assertEqual(compound.TypeId, "Part::Compound")
        self.assertEqual([link.Name for link in compound.Links], ["BoxA", "BoxB"])
        self.assertTrue(a.Visibility, "a compound leaves its links visible")
        self.assertEqual(len(compound.Shape.Solids), 2)

        # One compound is a legal fuse selection: the object type flattens it.
        self.select(compound)
        self.assertTrue(_enabled("Part_Fuse"))
        FreeCADGui.runCommand("Part_Fuse", 0)
        fusion = self.doc.getObject("Fusion")
        self.assertIsNotNone(fusion)
        self.assertAlmostEqual(_volume(fusion), 1500.0, places=6)

    def test_the_first_operand_supplies_the_appearance(self):
        a = _box("BoxA")
        a.Color = [(1.0, 0.0, 0.0, 1.0)]
        a.DisplayMode = "Wireframe"
        b = _box("BoxB", x=5.0)
        self.select(a, b)
        FreeCADGui.runCommand("Part_Fuse", 0)
        fusion = self.doc.getObject("Fusion")
        self.assertEqual(list(fusion.Color[0]), [1.0, 0.0, 0.0, 1.0])
        self.assertEqual(fusion.DisplayMode, "Wireframe")

    def test_compound_keeps_selection_order_without_repeats(self):
        a = _box("BoxA")
        b = _box("BoxB", x=5.0)
        FreeCADGui.Selection.clearSelection()
        FreeCADGui.Selection.addSelection(b)
        FreeCADGui.Selection.addSelection(a)
        FreeCADGui.Selection.addSelection(b)
        FreeCADGui.runCommand("Part_Compound", 0)
        compound = self.doc.getObject("Compound")
        self.assertEqual([link.Name for link in compound.Links], ["BoxB", "BoxA"])


class RefineTests(unittest.TestCase):
    """`Refine` — upstream's default-on unify pass — reads the property."""

    def setUp(self):
        self.doc = FreeCAD.newDocument("RefineTests")

    def tearDown(self):
        if FreeCAD.getDocument(self.doc.Name) is not None:
            FreeCAD.closeDocument(self.doc.Name)

    def fuse_face_to_face(self):
        """Two 10 mm boxes touching along X: a seam the fuse leaves behind."""
        base = _box("BoxA")
        tool = _box("BoxB", x=10.0)
        cut = self.doc.addObject("Part::Fuse", "Seam")
        cut.Base = base
        cut.Tool = tool
        self.doc.recompute()
        return cut

    def test_refine_is_on_by_default_and_unifies_the_seam(self):
        fusion = self.fuse_face_to_face()
        self.assertEqual(fusion.Refine, True)
        self.assertEqual(
            len(fusion.Shape.Faces), 6, "the coplanar pairs are one face again"
        )
        self.assertAlmostEqual(_volume(fusion), 2000.0, places=6)

    def test_refine_off_keeps_the_seam(self):
        fusion = self.fuse_face_to_face()
        fusion.Refine = False
        self.doc.recompute()
        self.assertEqual(len(fusion.Shape.Faces), 10, "the seam survives")
        self.assertAlmostEqual(_volume(fusion), 2000.0, places=6)


class MultiCommonBehaviorTests(unittest.TestCase):
    def setUp(self):
        self.doc = FreeCAD.newDocument("MultiCommonBehaviorTests")

    def tearDown(self):
        if FreeCAD.getDocument(self.doc.Name) is not None:
            FreeCAD.closeDocument(self.doc.Name)

    def test_behavior_decides_whether_a_lone_compound_is_flattened(self):
        a = _box("BoxA")
        b = _box("BoxB", x=5.0)
        compound = self.doc.addObject("Part::Compound", "Pair")
        compound.Links = [a, b]
        common = self.doc.addObject("Part::MultiCommon", "Common")
        common.Shapes = [compound]
        self.doc.recompute()

        self.assertEqual(common.Behavior, "CommonOfAllShapes")
        # The lone compound is expanded, so this is BoxA ∩ BoxB — the 5 × 10 × 10
        # overlap.
        self.assertAlmostEqual(common.Shape.Volume, 500.0, places=6)

        common.Behavior = "CommonOfFirstAndRest"
        self.doc.recompute()
        # The compound is taken whole instead, and the common of one shape is that
        # shape: the two disjoint halves of the pair (2000 mm³, no shared volume).
        self.assertAlmostEqual(common.Shape.Volume, 2000.0, places=6)
        self.assertEqual(len(common.Shape.Solids), 2)


if __name__ == "__main__":
    unittest.main()
