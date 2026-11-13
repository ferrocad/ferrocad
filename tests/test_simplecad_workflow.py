"""The SimpleCAD scripted workflow acceptance (TODO-13 P1 SC7).

Draw a profile on a face, push/pull it, cut a hole, and export the result:

    makeBox
      → generalFuse with a line that splits the top face into two profiles
      → extrude one profile (the push/pull) and fuse it onto the base
      → cut a square hole through the boss and the base
      → exportStl, read it back with Mesh.Mesh, check the volume

Every step asserts the shape kind, validity/closure and its pinned volume. The
numbers are exact for this scenario because every face is planar, so the STL
read-back can be compared to the B-Rep volume directly rather than with a
tessellation tolerance.

This module is in the ``workbenches`` extension set in
``contracts/extension-sets.json``: it needs the OCCT-backed ``Part`` and ``Mesh``.
Run it with ``tools/run_tests.py --extension workbenches``.
"""

from __future__ import annotations

import os
import tempfile
import unittest

import Mesh
import Part
from FreeCAD import Vector

#: The base block, in millimetres.
BASE = (40.0, 30.0, 10.0)
#: How far the split top profile is pushed/pulled, in millimetres.
PUSH_PULL = 5.0
#: The square hole (x, y, depth), cut from z=0 through the boss at z=15.
HOLE = (8.0, 8.0, 15.0)
HOLE_ORIGIN = (6.0, 11.0, 0.0)

BASE_VOLUME = 12_000.0
PROFILE_AREA = 600.0  # Half of the 40 × 30 top, after the split line at x=20.
BOSS_VOLUME = 3_000.0
FUSED_VOLUME = 15_000.0
HOLE_VOLUME = 960.0
FINAL_VOLUME = 14_040.0
MESH_FACETS = 44


def top_faces(shape):
    """The +Z faces, left to right; the split produces exactly two."""
    return sorted(
        (face for face in shape.Faces if face.normalAt(0.0, 0.0).z > 0.99),
        key=lambda face: face.CenterOfMass.x,
    )


class SimpleCadWorkflowTests(unittest.TestCase):
    def test_draw_extrude_push_pull_cut_export(self):
        # 1. The base solid. A box has six planar faces and a positive, closed volume.
        base = Part.makeBox(*BASE)
        self.assertEqual(base.ShapeType, "Solid")
        self.assertTrue(base.isValid())
        self.assertTrue(base.isClosed())
        self.assertAlmostEqual(base.Volume, BASE_VOLUME, places=9)

        # 2. Draw a profile on the top face: a line across it splits one face into
        #    two. The general fuse keeps the solid's volume and reports one split
        #    list per input.
        line = Part.makeLine(Vector(20.0, 0.0, 10.0), Vector(20.0, 30.0, 10.0))
        self.assertEqual(line.ShapeType, "Edge")
        self.assertTrue(line.isValid())
        split_shape, split_map = base.generalFuse([line])
        self.assertEqual(len(split_map), 2, "one split list per input")
        self.assertTrue(split_shape.isValid())
        self.assertAlmostEqual(split_shape.Volume, BASE_VOLUME, places=9)
        profiles = top_faces(split_shape)
        self.assertEqual(len(profiles), 2, "the line splits the top into two profiles")
        for profile in profiles:
            self.assertEqual(profile.ShapeType, "Face")
            self.assertAlmostEqual(profile.Area, PROFILE_AREA, places=9)

        # 3. Push/pull the left profile upward. A planar face extrudes to a closed
        #    solid, and fusing it onto the base adds exactly its volume.
        profile = profiles[0]
        self.assertLess(profile.CenterOfMass.x, 20.0)
        boss = profile.extrude(Vector(0.0, 0.0, PUSH_PULL))
        self.assertEqual(boss.ShapeType, "Solid")
        self.assertTrue(boss.isValid())
        self.assertTrue(boss.isClosed())
        self.assertAlmostEqual(boss.Volume, BOSS_VOLUME, places=9)

        fused = split_shape.Solids[0].fuse(boss)
        self.assertEqual(len(fused.Solids), 1, "the boss and the base are one solid")
        fused_solid = fused.Solids[0]
        self.assertEqual(fused_solid.ShapeType, "Solid")
        self.assertTrue(fused_solid.isValid())
        self.assertTrue(fused_solid.isClosed())
        self.assertAlmostEqual(fused_solid.Volume, FUSED_VOLUME, places=9)

        # 4. Cut a hole straight through the boss and the base. The square hole is
        #    inside the boss footprint, so it removes its whole 8 × 8 × 15 volume.
        hole = Part.makeBox(*HOLE, Vector(*HOLE_ORIGIN))
        self.assertAlmostEqual(hole.Volume, HOLE_VOLUME, places=9)
        cut = fused_solid.cut(hole)
        self.assertEqual(len(cut.Solids), 1, "the cut leaves one connected solid")
        final = cut.Solids[0]
        self.assertEqual(final.ShapeType, "Solid")
        self.assertTrue(final.isValid())
        self.assertTrue(final.isClosed())
        self.assertAlmostEqual(final.Volume, FINAL_VOLUME, places=6)

        # 5. Export and read back. Every face is planar, so the mesh volume equals
        #    the B-Rep volume without a deflection allowance.
        with tempfile.TemporaryDirectory(prefix="simplecad-workflow-") as folder:
            path = os.path.join(folder, "workflow.stl")
            self.assertIsNone(final.exportStl(path))
            self.assertGreater(os.path.getsize(path), 84)
            mesh = Mesh.Mesh(path)
            self.assertEqual(mesh.CountFacets, MESH_FACETS)
            self.assertAlmostEqual(mesh.Volume, FINAL_VOLUME, places=6)
            self.assertAlmostEqual(mesh.Volume, final.Volume, places=6)


if __name__ == "__main__":
    unittest.main()
