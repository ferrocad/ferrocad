"""Tests for the minimal ``Draft`` compatibility shim (``python/Draft``).

The shim exists so the upstream Part tests (``TestPartMirror.py`` and, through
it, ``TestPartApp.py``) can *load*: they only need ``Draft.make_clone``. It also
implements the scripted ``Draft ShapeString`` (``make_shape_string`` and its
deprecated aliases), whose geometry needs ``Part`` but whose module import does
not. It is deliberately not the Draft workbench, so these tests pin down the
implemented surface and the loud failures outside it.

``make_clone`` builds an ``App::Link``. In this standalone Python environment
the ``Part`` module is not loaded, so the link has no ``Shape`` output; the
Shape/placement recompute path is exercised by the Part conformance run
(``cargo test -p ferrocad_part_py --test part_conformance``).

Run::

    PYTHONPATH=python python3 -m pytest tests -q
"""

from __future__ import annotations

import unittest
from unittest import mock

import FreeCAD as App

import Draft


class _NoActiveDocument:
    """Stand-in for the ``FreeCAD`` module when no document is open."""

    ActiveDocument = None


class DraftShimSurfaceTests(unittest.TestCase):
    def test_the_implemented_surface_is_exported(self):
        self.assertEqual(
            Draft.__all__,
            [
                "make_clone",
                "make_shape_string",
                "make_shapestring",
                "makeShapeString",
                "ShapeString",
            ],
        )
        for name in Draft.__all__:
            self.assertTrue(callable(getattr(Draft, name)), name)

    def test_a_missing_draft_name_raises_and_names_the_shim(self):
        with self.assertRaises(AttributeError) as caught:
            Draft.make_circle  # noqa: B018 - attribute access is the assertion
        message = str(caught.exception)
        self.assertIn("compatibility shim", message)
        self.assertIn("make_clone", message)


class MakeCloneTests(unittest.TestCase):
    def setUp(self):
        self.doc = App.newDocument("DraftShimTest")
        self.source = self.doc.addObject("App::FeaturePython", "Source")
        self.source.Placement = App.Placement(
            App.Vector(1, 2, 3), App.Rotation(App.Vector(0, 0, 1), 30)
        )
        self.addCleanup(App.closeDocument, self.doc.Name)

    def test_returns_a_linked_object_in_the_active_document(self):
        clone = Draft.make_clone(self.source)

        self.assertIs(self.doc.getObject(clone.Name), clone)
        self.assertEqual(clone.TypeId, "App::Link")
        self.assertEqual(clone.LinkedObject.Name, "Source")

    def test_starts_at_the_source_placement(self):
        clone = Draft.make_clone(self.source)

        self.assertEqual(clone.Placement.Base, App.Vector(1, 2, 3))
        self.assertAlmostEqual(clone.Placement.Rotation.Angle, 0.5235987755982988)

    def test_placement_is_settable_and_survives_recompute(self):
        clone = Draft.make_clone(self.source)
        clone.Placement = App.Placement(
            App.Vector(30, 0, 0), App.Rotation(App.Vector(0, 0, 1), 90)
        )
        self.doc.recompute()

        self.assertEqual(clone.Placement.Base, App.Vector(30, 0, 0))
        self.assertAlmostEqual(clone.Placement.Rotation.Angle, 1.5707963267948966)

    def test_delta_offsets_the_clone(self):
        clone = Draft.make_clone(self.source, delta=App.Vector(10, -1, 0))

        self.assertEqual(clone.Placement.Base, App.Vector(11, 1, 3))

    def test_a_one_element_list_is_accepted(self):
        clone = Draft.make_clone([self.source])

        self.assertEqual(clone.LinkedObject.Name, "Source")

    def test_several_objects_are_not_silently_dropped(self):
        with self.assertRaises(NotImplementedError) as caught:
            Draft.make_clone([self.source, self.source])
        self.assertIn("exactly one object", str(caught.exception))

    def test_no_active_document_is_an_error(self):
        with mock.patch.object(Draft, "App", _NoActiveDocument()):
            with self.assertRaises(RuntimeError) as caught:
                Draft.make_clone(self.source)
        self.assertIn("newDocument", str(caught.exception))


if __name__ == "__main__":
    unittest.main()
