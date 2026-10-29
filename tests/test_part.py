"""The Part workbench module (``import Part``).

These run against a build that includes ``ferrocad_part_py`` in the same
extension as the ``ferrocad`` bindings (the app / a wheel that bundles Part). The
standalone ``ferrocad.abi3.so`` produced by ``build.sh`` does not, because a
Python extension module cannot share its process-global core registry with a
second ``.so``; the composition inside one extension is covered by
``crates/ferrocad_part_py/tests/part_boot.rs``.

This module is in the ``workbenches`` suite in
``contracts/extension-sets.json``: it is never run in the base extension set, so
it imports Part directly. Run it with
``tools/run_tests.py --extension workbenches``; plain ``unittest discover`` in a
base-only checkout is not a supported way to run the suites.
"""

from __future__ import annotations

import tempfile
import unittest

import Part


class MakeBoxTests(unittest.TestCase):
    def test_make_box_returns_a_shape(self):
        shape = Part.makeBox(2, 3, 4)
        self.assertIsInstance(shape, Part.Shape)
        self.assertFalse(shape.isNull())

    def test_a_default_shape_is_null(self):
        self.assertTrue(Part.Shape().isNull())

    def test_exported_brep_is_not_empty(self):
        self.assertTrue(Part.makeBox(1, 1, 1).exportBrepToString())


class TessellationTests(unittest.TestCase):
    def test_angular_deflection_changes_the_triangle_count(self):
        # A fresh shape per call: OCCT stores the triangulation on the shape and
        # reuses it when the next request is not finer.
        coarse = Part.makeSphere(10).tessellate(2.0, 2.0)[1]
        fine = Part.makeSphere(10).tessellate(2.0, 1.0)[1]
        tighter = Part.makeSphere(10).tessellate(0.5, 0.2)[1]
        self.assertLess(len(coarse), len(fine))
        self.assertLess(len(fine), len(tighter))

    def test_a_non_positive_angular_deflection_is_rejected(self):
        shape = Part.makeSphere(10)
        for angular in (0.0, -0.5):
            with self.assertRaises(Exception):
                shape.tessellate(1.0, angular)


class FaceTests(unittest.TestCase):
    def test_a_face_from_an_open_wire_is_invalid(self):
        # Wires may be open (most sketches are), but only a closed wire bounds a
        # valid face. OCCT's MakeFace still returns a face; it is marked invalid and
        # cannot be used in a solid. This is the behaviour the introduction describes.
        bottom = Part.makeLine((0, 0, 0), (10, 0, 0))
        side = Part.makeLine((10, 0, 0), (10, 10, 0))
        open_wire = Part.Wire([bottom, side])
        self.assertFalse(open_wire.isClosed())
        self.assertFalse(Part.Face(open_wire).isValid())

        top = Part.makeLine((10, 10, 0), (0, 10, 0))
        left = Part.makeLine((0, 10, 0), (0, 0, 0))
        closed_wire = Part.Wire([bottom, side, top, left])
        self.assertTrue(closed_wire.isClosed())
        self.assertTrue(Part.Face(closed_wire).isValid())


class ShapePropertyTests(unittest.TestCase):
    def test_a_part_feature_round_trips_its_shape(self):
        import FreeCAD

        doc = FreeCAD.newDocument("PartShapeTests")
        try:
            obj = doc.addObject("Part::Feature", "Box")
            self.assertTrue(obj.Shape.isNull())
            box = Part.makeBox(10, 10, 10)
            obj.Shape = box
            self.assertFalse(obj.Shape.isNull())
            self.assertTrue(box.isSame(obj.Shape))
        finally:
            FreeCAD.closeDocument(doc.Name)

    def test_the_shape_survives_a_save_and_reload(self):
        import FreeCAD

        doc = FreeCAD.newDocument("PartPersistenceTests")
        reopened = None
        with tempfile.TemporaryDirectory() as tmp:
            path = f"{tmp}/box.ferrocad"
            try:
                obj = doc.addObject("Part::Feature", "Box")
                obj.Shape = Part.makeBox(1, 2, 3)
                doc.saveAs(path)

                reopened = FreeCAD.openDocument(path)
                self.assertFalse(reopened.getObject("Box").Shape.isNull())
            finally:
                FreeCAD.closeDocument(doc.Name)
                if reopened is not None:
                    FreeCAD.closeDocument(reopened.Name)


if __name__ == "__main__":
    unittest.main()
