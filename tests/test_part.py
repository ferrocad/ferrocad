"""The Part workbench module (``import Part``).

These run against a build that includes ``ferrocad_part_py`` in the same image as
the ``ferrocad`` bindings (the app / a wheel that bundles Part). The standalone
``ferrocad.abi3.so`` produced by ``build.sh`` does not, because a Python extension
module cannot share its process-global core registry with a second ``.so``; the
in-image composition is covered by
``crates/ferrocad_part_py/tests/part_boot.rs``.

This module is in the ``kernel`` suite in ``contracts/test-images.json``: it is
never run in the app-only image, so it imports Part directly. Run it with
``tools/run_tests.py --image combined``; plain ``unittest discover`` in an
app-only checkout is not a supported way to run the suites.
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
