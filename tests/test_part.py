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


class BSplineSurfaceTests(unittest.TestCase):
    POLES = [
        [[0, 0, 0], [0, 3, 2], [0, 6, 2], [0, 9, 0]],
        [[3, 0, 2], [3, 3, 3], [3, 6, 3], [3, 9, 2]],
        [[6, 0, 2], [6, 3, 3], [6, 6, 3], [6, 9, 2]],
        [[9, 0, 0], [9, 3, 2], [9, 6, 2], [9, 9, 0]],
    ]

    def test_a_clamped_bicubic_patch(self):
        # The how-to guide's example: 4x4 poles, degree 3, clamped knots.
        surface = Part.BSplineSurface()
        surface.buildFromPolesMultsKnots(
            self.POLES, [4, 4], [4, 4], [0.0, 1.0], [0.0, 1.0], False, False, 3, 3
        )
        self.assertEqual(surface.TypeId, "Part::GeomBSplineSurface")
        self.assertEqual((surface.UDegree, surface.VDegree), (3, 3))
        self.assertEqual(len(surface.getPoles()), 4)
        self.assertEqual(len(surface.getPoles()[0]), 4)
        self.assertEqual(surface.getUKnots(), [0.0, 1.0])
        self.assertEqual(surface.getUMultiplicities(), [4, 4])
        self.assertEqual(surface.getWeights()[0], [1.0, 1.0, 1.0, 1.0])

        face = surface.toShape()
        self.assertTrue(face.isValid())
        self.assertAlmostEqual(face.Area, 87.45840756478856, places=6)
        self.assertEqual(face.Surface.TypeId, "Part::GeomBSplineSurface")

    def test_an_interpolated_grid_builds_a_face(self):
        # interpolate keeps the grid, so it can build the face and report
        # bounds/knot sequences; the pole and degree getters need an explicit build.
        surface = Part.BSplineSurface()
        surface.interpolate(
            [
                [[0, 0, 0], [0, 3, 1], [0, 6, 0], [0, 9, 1]],
                [[3, 0, 1], [3, 3, 2], [3, 6, 2], [3, 9, 1]],
                [[6, 0, 0], [6, 3, 1], [6, 6, 1], [6, 9, 0]],
            ]
        )
        self.assertEqual(surface.bounds(), (0.0, 1.0, 0.0, 1.0))
        self.assertEqual(len(surface.UKnotSequence), 9)
        face = surface.toShape()
        self.assertTrue(face.isValid())
        self.assertGreater(face.Area, 0.0)


    def test_a_flat_bilinear_patch(self):
        surface = Part.BSplineSurface()
        surface.buildFromPolesMultsKnots(
            [[[0, 0, 0], [2, 0, 0]], [[0, 1, 0], [2, 1, 0]]],
            [2, 2], [2, 2], [0.0, 1.0], [0.0, 1.0], False, False, 1, 1,
        )
        self.assertEqual((surface.UDegree, surface.VDegree), (1, 1))
        face = surface.toShape()
        self.assertTrue(face.isValid())
        self.assertAlmostEqual(face.Area, 2.0, places=9)

    def test_a_wavy_sheet_from_a_generated_grid(self):
        surface = self.wavy_surface()
        face = surface.toShape()
        self.assertTrue(face.isValid())
        self.assertAlmostEqual(face.Area, 83.071692, places=5)

    def test_a_periodic_tube(self):
        surface = self.tube_surface()
        self.assertTrue(surface.isVPeriodic())
        face = surface.toShape()
        self.assertTrue(face.isValid())
        self.assertAlmostEqual(face.Area, 67.995767, places=5)

    @staticmethod
    def wavy_surface():
        import math

        poles = [
            [[u, v * 3.0, math.sin(2 * math.pi * u / 8) * 2.0] for v in range(4)]
            for u in range(8)
        ]
        surface = Part.BSplineSurface()
        surface.buildFromPolesMultsKnots(
            poles,
            [4, 2, 2, 4], [4, 4],
            [0.0, 1 / 3, 2 / 3, 1.0], [0.0, 1.0],
            False, False, 3, 3,
        )
        return surface

    @staticmethod
    def tube_surface():
        import math

        poles = [
            [
                [
                    2.0 * math.cos(2 * math.pi * v / 8),
                    2.0 * math.sin(2 * math.pi * v / 8),
                    u * 2.0,
                ]
                for v in range(8)
            ]
            for u in range(4)
        ]
        surface = Part.BSplineSurface()
        surface.buildFromPolesMultsKnots(
            poles,
            [4, 4], [1] * 9,
            [0.0, 1.0], [index / 8 for index in range(9)],
            False, True, 3, 3,
        )
        return surface

    def test_extruding_a_face_makes_a_solid(self):
        slab = self.wavy_surface().toShape().extrude((0.0, 0.0, 3.0))
        self.assertEqual(slab.ShapeType, "Solid")
        self.assertTrue(slab.isValid())
        self.assertAlmostEqual(slab.Volume, 190.4042448629122, places=5)

    def test_capping_a_closed_surface_makes_a_solid(self):
        tube = self.tube_surface().toShape()
        caps = [Part.Face(Part.Wire([edge])) for edge in tube.Edges if edge.isClosed()]
        self.assertEqual(len(caps), 2)
        shell = Part.makeShell([tube.reversed(), *caps])
        solid = Part.makeSolid(shell)
        self.assertEqual(solid.ShapeType, "Solid")
        self.assertTrue(solid.isValid())
        self.assertAlmostEqual(solid.Volume, 61.31958336992129, places=5)


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
