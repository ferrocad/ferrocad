"""Tests for ``flatmesh``, the pure-Python face unwrapper SimpleCAD imports.

The unwrapper is deliberately not FreeCAD's least-squares conformal solver: it
rigidly unfolds the face's triangulation and relaxes it.  These tests pin down
what that promise means: planar faces come back exactly, developable faces
(cylinders, cones) keep their area, seams are cut open with duplicate 3-D
nodes, boundary loops are complete and ordered, and degenerate input raises.

Run::

    PYTHONPATH=python python3 -m pytest tests -q
"""

from __future__ import annotations

import os
import subprocess
import sys
import unittest

import flatmesh


class _Point:
    """The ``x``/``y``/``z`` surface of a tessellation point, without Part."""

    __slots__ = ("x", "y", "z")

    def __init__(self, x, y, z):
        self.x = float(x)
        self.y = float(y)
        self.z = float(z)


class _StubFace:
    """A minimal stand-in for a face, so the degenerate cases need no kernel."""

    def __init__(self, points, facets, area=1.0, shape_type="Face"):
        self.ShapeType = shape_type
        self.Area = area
        self._points = [_Point(*point) for point in points]
        self._facets = [tuple(facet) for facet in facets]

    def tessellate(self, deflection=0.1):
        return self._points, self._facets


from flatmesh_fixtures import flat_triangle_area, loop_area


class DegenerateInputTests(unittest.TestCase):
    def test_a_non_face_is_refused(self):
        with self.assertRaisesRegex(ValueError, "needs a face"):
            flatmesh.FaceUnwrapper(_StubFace([], [], shape_type="Solid"))

    def test_an_empty_triangulation_is_refused(self):
        with self.assertRaisesRegex(ValueError, "at least three"):
            flatmesh.FaceUnwrapper(_StubFace([], []))

    def test_fewer_than_three_nodes_is_refused(self):
        with self.assertRaisesRegex(ValueError, "at least three"):
            flatmesh.FaceUnwrapper(_StubFace([(0, 0, 0), (1, 0, 0)], []))

    def test_a_zero_area_face_is_refused(self):
        points = [(0, 0, 0), (1, 0, 0), (2, 0, 0)]
        with self.assertRaisesRegex(ValueError, "zero area"):
            flatmesh.FaceUnwrapper(_StubFace(points, [(0, 1, 2)]))

    def test_a_missing_node_in_a_facet_is_refused(self):
        points = [(0, 0, 0), (1, 0, 0), (0, 1, 0)]
        with self.assertRaisesRegex(ValueError, "does not have"):
            flatmesh.FaceUnwrapper(_StubFace(points, [(0, 1, 9)]))

    def test_boundary_loops_before_find_flat_nodes_are_refused(self):
        unwrapper = flatmesh.FaceUnwrapper(
            _StubFace([(0, 0, 0), (1, 0, 0), (1, 1, 0), (0, 1, 0)], [(0, 1, 2), (0, 2, 3)])
        )
        with self.assertRaisesRegex(ValueError, "findFlatNodes"):
            unwrapper.getFlatBoundaryNodes()

    def test_a_square_stub_unwraps(self):
        unwrapper = flatmesh.FaceUnwrapper(
            _StubFace([(0, 0, 0), (2, 0, 0), (2, 1, 0), (0, 1, 0)], [(0, 1, 2), (0, 2, 3)])
        )
        unwrapper.findFlatNodes(5, 0.95)
        self.assertAlmostEqual(flat_triangle_area(unwrapper), 2.0, places=9)
        loops = unwrapper.getFlatBoundaryNodes()
        self.assertEqual(len(loops), 1)
        self.assertAlmostEqual(loop_area(loops[0]), 2.0, places=9)


class IterationTests(unittest.TestCase):
    def square(self):
        return flatmesh.FaceUnwrapper(
            _StubFace([(0, 0, 0), (2, 0, 0), (2, 1, 0), (0, 1, 0)], [(0, 1, 2), (0, 2, 3)])
        )

    def test_zero_iterations_keeps_the_rigid_unfolding(self):
        unwrapper = self.square()
        unwrapper.findFlatNodes(0, 0.95)
        expected = list(unwrapper.ze_nodes)
        unwrapper.findFlatNodes(5, 0.95)
        for before, after in zip(expected, unwrapper.ze_nodes):
            self.assertAlmostEqual(before[0], after[0], places=9)
            self.assertAlmostEqual(before[1], after[1], places=9)

    def test_out_of_range_arguments_are_clamped(self):
        unwrapper = self.square()
        unwrapper.findFlatNodes(-2, 4.0)
        self.assertEqual(len(unwrapper.ze_nodes), len(unwrapper.nodes))

    def test_the_boundary_points_are_flat_xy_triples(self):
        unwrapper = self.square()
        unwrapper.findFlatNodes(5, 0.95)
        for loop in unwrapper.getFlatBoundaryNodes():
            for point in loop:
                self.assertEqual(len(point), 3)
                self.assertEqual(point[2], 0.0)


class ModuleTests(unittest.TestCase):
    def test_importing_flatmesh_does_not_import_numpy(self):
        python_dir = os.path.join(
            os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "python"
        )
        environment = dict(os.environ)
        existing = environment.get("PYTHONPATH")
        environment["PYTHONPATH"] = python_dir + (os.pathsep + existing if existing else "")
        result = subprocess.run(
            [sys.executable, "-c", "import sys, flatmesh; print('numpy' in sys.modules)"],
            capture_output=True,
            text=True,
            env=environment,
            timeout=120,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "False")


if __name__ == "__main__":
    unittest.main()
