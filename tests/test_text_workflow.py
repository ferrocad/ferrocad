"""3D text: the outline → wire → face → solid workflow, and both ways to use it.

`Part.makeWireString` gives one list of closed *wires* per character. A bare wire
extrudes to an open **shell** (and a counter/hole wire extrudes to a shell with
negative volume); only a **face** extrudes to a solid. So the workflow is:

    wires → Part.Face(wires, "Part::FaceMakerBullseye")   # outer + holes
          → face.extrude(Vector(0, 0, depth))             # one solid per glyph
          → multiFuse                                      # raised text
          → plate.cut(...)                                 # etched text

The text itself is not a kernel concept: `ferrocad_text` turns a font's glyphs
into contours with no OCCT, and the Part workbench turns those contours into
B-Rep. See `docs/project/tasks/text-and-3d-text.md` for why the toy edition
cannot do the second half, and why there is no 72/96 DPI anywhere.

This module is in the Part extension set; it needs the OCCT-backed `Part`.
"""

from __future__ import annotations

import os
import pathlib
import tempfile
import unittest

import Draft
import FreeCAD
import Part
from FreeCAD import Vector

FONT_DIR = (
    pathlib.Path(__file__).resolve().parents[1]
    / "crates"
    / "ferrocad_text"
    / "tests"
    / "fixtures"
    / "fonts"
)
FONT = "LiberationMono-Regular.ttf"

#: The height upstream's scale is given in millimetres. At 100 the cap height is
#: ~5.8 mm (the scale is `height / (10 * line_height)`, recorded in SC2).
HEIGHT = 100.0
#: The extrusion depth, in millimetres.
DEPTH = 2.0


def letters(text: str = "ferrocad", height: float = HEIGHT):
    """One list of closed wires per character."""
    return Part.makeWireString(text, str(FONT_DIR) + os.sep, FONT, height)


def faces(text: str = "ferrocad"):
    """One planar face per glyph, with its counters as holes."""
    return [
        face
        for wires in letters(text)
        for face in Part.Face(wires, "Part::FaceMakerBullseye").Faces
    ]


class TextGeometryTests(unittest.TestCase):
    def test_a_bare_wire_extrudes_to_an_open_shell(self):
        """The reason a solid needs a face, not just a wire."""
        outer, _hole = letters("o")[0]
        shell = outer.extrude(Vector(0, 0, DEPTH))
        self.assertEqual(shell.ShapeType, "Shell")
        self.assertFalse(shell.isClosed())
        self.assertEqual(len(shell.Solids), 0)

    def test_a_face_with_holes_extrudes_to_a_closed_solid(self):
        face = Part.Face(letters("o")[0], "Part::FaceMakerBullseye")
        self.assertEqual(face.ShapeType, "Face")
        self.assertEqual(len(face.Wires), 2, "the 'o' has an outer wire and a counter")
        solid = face.extrude(Vector(0, 0, DEPTH))
        self.assertEqual(solid.ShapeType, "Solid")
        self.assertTrue(solid.isClosed())
        self.assertAlmostEqual(solid.Volume, face.Area * DEPTH, places=9)

    def test_lowercase_ferrocad_has_eight_glyphs_and_four_counters(self):
        contours = [len(wires) for wires in letters()]
        self.assertEqual(len(contours), 8)
        self.assertEqual(
            contours, [1, 2, 1, 1, 2, 1, 2, 2], "e, o, a and d each have a counter"
        )

    def test_every_glyph_extrudes_to_one_valid_solid(self):
        solids = [face.extrude(Vector(0, 0, DEPTH)) for face in faces()]
        self.assertEqual(len(solids), 8)
        for solid in solids:
            self.assertEqual(solid.ShapeType, "Solid")
            self.assertTrue(solid.isValid())
            self.assertTrue(solid.isClosed())


class TextIntoAPlateTests(unittest.TestCase):
    """Raised text adds its volume; etched text removes the overlapping part."""

    def setUp(self):
        self.solids = [face.extrude(Vector(0, 0, DEPTH)) for face in faces()]
        # Disjoint letters fuse to a compound, which is what a boolean takes.
        self.text = self.solids[0].multiFuse(self.solids[1:])
        bound = self.text.BoundBox
        # 2 mm margins, and the baseline overshoot of the round letters (o, e, c)
        # is below y = 0, so the shift uses the bounding box, not the baseline.
        self.shift = Vector(-bound.XMin + 2, -bound.YMin + 2, 0)
        self.plate = Part.makeBox(bound.XLength + 4, bound.YLength + 4, 3)

    def test_raised_text_adds_exactly_its_volume(self):
        placed = self.text.copy()
        placed.translate(self.shift + Vector(0, 0, 3))
        raised = self.plate.fuse(placed)
        self.assertTrue(raised.isValid())
        self.assertAlmostEqual(
            raised.Volume, self.plate.Volume + self.text.Volume, places=6
        )

    def test_etched_text_removes_the_overlapping_volume(self):
        # 2 mm of text, 1 mm of it inside the plate's 3 mm thickness.
        placed = self.text.copy()
        placed.translate(self.shift + Vector(0, 0, 2))
        etched = self.plate.cut(placed)
        self.assertTrue(etched.isValid())
        self.assertAlmostEqual(
            etched.Volume, self.plate.Volume - self.text.Volume / 2, places=4
        )

    def test_translate_mutates_in_place(self):
        """Why the two placements copy first: `Shape.translate` returns itself."""
        before = self.text.Volume
        moved = self.text.translate(Vector(0, 0, 10))
        self.assertIs(moved, self.text)
        self.assertAlmostEqual(moved.Volume, before, places=9)
        self.assertAlmostEqual(moved.BoundBox.ZMin, 10.0, places=9)


class DraftShapeStringTests(unittest.TestCase):
    """Upstream's object form: `Draft.make_shape_string`."""

    def setUp(self):
        self.doc = FreeCAD.newDocument("ShapeStringTests")

    def tearDown(self):
        if self.doc.Name in FreeCAD.listDocuments():
            FreeCAD.closeDocument(self.doc.Name)

    def make(self, text="ferrocad", size=20.0, tracking=1.0):
        return Draft.make_shape_string(text, str(FONT_DIR / FONT), size, tracking)

    def test_it_builds_one_face_per_glyph(self):
        obj = self.make()
        self.assertEqual(obj.String, "ferrocad")
        self.assertEqual(len(obj.Shape.Faces), 8)
        self.assertTrue(obj.Shape.isValid())
        for face in obj.Shape.Faces:
            solid = face.extrude(Vector(0, 0, DEPTH))
            self.assertEqual(solid.ShapeType, "Solid")
            self.assertTrue(solid.isClosed())

    def test_scale_to_size_makes_size_the_cap_height(self):
        """Upstream's `ScaleToSize` plus `Bottom-Left`/`Cap Height`."""
        obj = self.make("M", size=20.0, tracking=0.0)
        box = obj.Shape.BoundBox
        self.assertAlmostEqual(box.YMin, 0.0, places=6)
        self.assertAlmostEqual(box.YMax, 20.0, places=6)
        self.assertAlmostEqual(box.XMin, 0.0, places=6)

    def test_make_face_off_gives_wires_and_shells(self):
        obj = self.make("o")
        obj.MakeFace = False
        self.doc.recompute()
        self.assertEqual(len(obj.Shape.Faces), 0)
        self.assertEqual(len(obj.Shape.Wires), 2)
        shell = obj.Shape.Wires[0].extrude(Vector(0, 0, DEPTH))
        self.assertEqual(shell.ShapeType, "Shell")

    def test_editing_the_string_rebuilds(self):
        obj = self.make()
        obj.String = "AB"
        self.doc.recompute()
        self.assertEqual(len(obj.Shape.Faces), 2)

    def test_the_deprecated_aliases_return_the_same_object(self):
        obj = Draft.make_shapestring("A", str(FONT_DIR / FONT), 10, 0)
        self.assertEqual(obj.String, "A")
        other = Draft.makeShapeString("B", str(FONT_DIR / FONT), 10, 0)
        self.assertEqual(other.String, "B")

    def test_it_round_trips_through_a_native_save(self):
        obj = self.make()
        self.assertEqual(len(obj.Shape.Faces), 8)
        with tempfile.TemporaryDirectory() as tmp:
            path = f"{tmp}/text.ferrocad"
            self.doc.saveAs(path)
            FreeCAD.closeDocument(self.doc.Name)
            reopened = FreeCAD.openDocument(path)
            try:
                copy = reopened.getObject("ShapeString")
                self.assertIsNotNone(copy)
                self.assertEqual(copy.String, "ferrocad")
                self.assertEqual(len(copy.Shape.Faces), 8)
                # The proxy came back, so an edit rebuilds the shape.
                copy.String = "A"
                reopened.recompute()
                self.assertEqual(len(copy.Shape.Faces), 1)
            finally:
                FreeCAD.closeDocument(reopened.Name)


if __name__ == "__main__":
    unittest.main()
