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

try:
    import Part
    from FreeCAD import Vector

    HAVE_PART = True
except ImportError:
    HAVE_PART = False


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


def _flat_triangle_area(unwrapper):
    """The area of the flat triangles, so a folding overlap would double-count."""
    total = 0.0
    for a, b, c in unwrapper.tris:
        ax, ay = unwrapper.ze_nodes[a][:2]
        bx, by = unwrapper.ze_nodes[b][:2]
        cx, cy = unwrapper.ze_nodes[c][:2]
        total += abs((bx - ax) * (cy - ay) - (by - ay) * (cx - ax)) / 2.0
    return total


def _loop_area(loop):
    return abs(
        sum(
            loop[i][0] * loop[i + 1][1] - loop[i + 1][0] * loop[i][1]
            for i in range(len(loop) - 1)
        )
    ) / 2.0


def _boundary_edges(unwrapper):
    """Undirected triangle edges that appear exactly once, by node index."""
    counts = {}
    for triangle in unwrapper.tris:
        for k in range(3):
            a, b = triangle[k], triangle[(k + 1) % 3]
            key = (a, b) if a < b else (b, a)
            counts[key] = counts.get(key, 0) + 1
    return {edge for edge, count in counts.items() if count == 1}


def _extent(unwrapper):
    xs = [point[0] for point in unwrapper.ze_nodes]
    ys = [point[1] for point in unwrapper.ze_nodes]
    return sorted((max(xs) - min(xs), max(ys) - min(ys)))


@unittest.skipUnless(HAVE_PART, "Part is not in this extension build")
class PlanarFaceTests(unittest.TestCase):
    def plate(self, width=10.0, height=4.0):
        box = Part.makeBox(width, height, 3)
        return next(face for face in box.Faces if abs(face.Area - width * height) < 1e-9)

    def unwrap(self, face):
        unwrapper = flatmesh.FaceUnwrapper(face.copy())
        unwrapper.findFlatNodes(5, 0.95)
        return unwrapper

    def test_a_planar_face_keeps_its_area_and_outline(self):
        face = self.plate()
        unwrapper = self.unwrap(face)

        self.assertAlmostEqual(_flat_triangle_area(unwrapper), face.Area, places=6)
        loops = unwrapper.getFlatBoundaryNodes()
        self.assertEqual(len(loops), 1)
        self.assertAlmostEqual(_loop_area(loops[0]), face.Area, places=6)
        # The sheet's orientation is the seed triangle's, so check the outline by
        # its side lengths: the rectangle must come back as 4 x 10.
        loop = loops[0]
        sides = sorted(
            ((loop[i + 1][0] - loop[i][0]) ** 2 + (loop[i + 1][1] - loop[i][1]) ** 2) ** 0.5
            for i in range(len(loop) - 1)
        )
        self.assertEqual(len(sides), 4)
        for side, expected in zip(sides, (4.0, 4.0, 10.0, 10.0)):
            self.assertAlmostEqual(side, expected, places=6)

    def test_the_third_column_is_zero_on_a_plane(self):
        unwrapper = self.unwrap(self.plate())
        self.assertEqual(len(unwrapper.ze_nodes), len(unwrapper.nodes))
        for point in unwrapper.ze_nodes:
            self.assertEqual(len(point), 3)
            self.assertAlmostEqual(point[2], 0.0, places=6)

    def test_a_face_with_a_hole_has_one_loop_per_boundary(self):
        plate = Part.makeBox(20, 20, 2)
        hole = Part.makeCylinder(4, 5, Vector(10, 10, -1))
        cut = plate.cut(hole)
        target = 400.0 - 3.141592653589793 * 16.0
        face = next(candidate for candidate in cut.Faces if abs(candidate.Area - target) < 1.0)

        unwrapper = self.unwrap(face)
        loops = unwrapper.getFlatBoundaryNodes()
        self.assertEqual(len(loops), 2)
        areas = sorted(_loop_area(loop) for loop in loops)
        self.assertAlmostEqual(areas[0], 3.141592653589793 * 16.0, delta=0.1)
        self.assertAlmostEqual(areas[1], 400.0, places=4)
        # The triangles tile the face with the hole removed.
        self.assertAlmostEqual(_flat_triangle_area(unwrapper), face.Area, delta=face.Area * 0.001)

    def test_the_nodes_are_the_tessellation_point_positions(self):
        face = self.plate()
        unwrapper = flatmesh.FaceUnwrapper(face)
        unwrapper.findFlatNodes(5, 0.95)
        points, facets = face.tessellate()
        self.assertEqual(len(unwrapper.tris), len(facets))
        expected = sorted(
            (round(point.x, 9), round(point.y, 9), round(point.z, 9)) for point in points
        )
        actual = sorted(
            (round(point[0], 9), round(point[1], 9), round(point[2], 9))
            for point in unwrapper.nodes
        )
        self.assertEqual(actual, expected)


@unittest.skipUnless(HAVE_PART, "Part is not in this extension build")
class DevelopableFaceTests(unittest.TestCase):
    """A developable face has no angle defect, so the rigid unfold is exact.

    The remaining difference from ``Face.Area`` is the triangulation: the
    boundary is a polygon inscribed in the smooth face.  The documented
    tolerance is 1% of the face's area.
    """

    def unwrap(self, face):
        unwrapper = flatmesh.FaceUnwrapper(face.copy())
        unwrapper.findFlatNodes(5, 0.95)
        return unwrapper

    def test_a_cylinder_wall_unwraps_to_its_area(self):
        part = Part.makeCylinder(10, 40)
        face = part.Faces[0]
        unwrapper = self.unwrap(face)

        self.assertAlmostEqual(_flat_triangle_area(unwrapper), face.Area, delta=face.Area * 0.01)
        loops = unwrapper.getFlatBoundaryNodes()
        self.assertEqual(len(loops), 1)
        self.assertAlmostEqual(_loop_area(loops[0]), face.Area, delta=face.Area * 0.01)
        short, long = _extent(unwrapper)
        self.assertAlmostEqual(short, 40.0, delta=0.1)
        self.assertAlmostEqual(long, 2 * 3.141592653589793 * 10.0, delta=0.2)

    def test_a_half_cylinder_wall_unwraps_to_its_area(self):
        part = Part.makeCylinder(20, 40, Vector(), Vector(0, 0, 1), 180)
        face = next(
            candidate for candidate in part.Faces if isinstance(candidate.Surface, Part.Cylinder)
        )
        unwrapper = self.unwrap(face)

        self.assertAlmostEqual(_flat_triangle_area(unwrapper), face.Area, delta=face.Area * 0.01)
        loops = unwrapper.getFlatBoundaryNodes()
        self.assertEqual(len(loops), 1)
        short, long = _extent(unwrapper)
        self.assertAlmostEqual(short, 40.0, delta=0.2)
        self.assertAlmostEqual(long, 3.141592653589793 * 20.0, delta=1.0)

    def test_a_cone_wall_unwraps_to_its_area(self):
        cone = Part.makeCone(10, 6, 20)
        face = cone.Faces[0]
        unwrapper = self.unwrap(face)
        self.assertAlmostEqual(_flat_triangle_area(unwrapper), face.Area, delta=face.Area * 0.01)
        self.assertEqual(len(unwrapper.getFlatBoundaryNodes()), 1)

    def test_the_seam_is_cut_into_two_columns(self):
        part = Part.makeCylinder(10, 40)
        face = part.Faces[0]
        unwrapper = self.unwrap(face)

        groups = {}
        for index, point in enumerate(unwrapper.nodes):
            key = (round(point[0], 6), round(point[1], 6), round(point[2], 6))
            groups.setdefault(key, []).append(index)
        duplicated = [indices for indices in groups.values() if len(indices) == 2]
        self.assertEqual(len(duplicated), 2, "the seam's two end nodes should be cut in two")

        seam = next(edge for edge in face.Edges if edge.isSeam(face))
        width = 2 * 3.141592653589793 * 10.0
        for indices in duplicated:
            for index in indices:
                point = unwrapper.nodes[index]
                distance = seam.distToShape(Part.Vertex(Vector(*point)))[0]
                self.assertLess(distance, 1e-6)
            first, second = (unwrapper.ze_nodes[index][:2] for index in indices)
            separation = ((first[0] - second[0]) ** 2 + (first[1] - second[1]) ** 2) ** 0.5
            self.assertAlmostEqual(separation, width, delta=width * 0.01)

    def test_the_seam_keeps_the_duplicated_points_identical_in_3d(self):
        part = Part.makeCylinder(10, 40)
        unwrapper = self.unwrap(part.Faces[0])
        groups = {}
        for point in unwrapper.nodes:
            key = (round(point[0], 6), round(point[1], 6), round(point[2], 6))
            groups[key] = groups.get(key, 0) + 1
        self.assertTrue(any(count == 2 for count in groups.values()))

    def test_a_curved_face_reports_a_nonzero_height(self):
        part = Part.makeCylinder(10, 40)
        unwrapper = self.unwrap(part.Faces[0])
        highest = max(abs(point[2]) for point in unwrapper.ze_nodes)
        self.assertGreater(highest, 1.0)


@unittest.skipUnless(HAVE_PART, "Part is not in this extension build")
class BoundaryLoopTests(unittest.TestCase):
    def unwrap(self, face):
        unwrapper = flatmesh.FaceUnwrapper(face.copy())
        unwrapper.findFlatNodes(5, 0.95)
        return unwrapper

    def assert_loops_are_ordered(self, unwrapper):
        """Each loop is closed and the loops together use every boundary edge once."""
        index_of = {}
        for index, point in enumerate(unwrapper.ze_nodes):
            index_of[(round(point[0], 9), round(point[1], 9))] = index

        walked = []
        for loop in unwrapper.getFlatBoundaryNodes():
            self.assertGreaterEqual(len(loop), 4)
            self.assertEqual(loop[0][:2], loop[-1][:2], "the loop must close on its first point")
            interior = {(round(p[0], 9), round(p[1], 9)) for p in loop[:-1]}
            self.assertEqual(len(interior), len(loop) - 1)
            for first, second in zip(loop, loop[1:]):
                a = index_of[(round(first[0], 9), round(first[1], 9))]
                b = index_of[(round(second[0], 9), round(second[1], 9))]
                walked.append((a, b) if a < b else (b, a))
        self.assertEqual(sorted(walked), sorted(_boundary_edges(unwrapper)))

    def test_a_cylinder_has_one_ordered_boundary(self):
        self.assert_loops_are_ordered(self.unwrap(Part.makeCylinder(10, 40).Faces[0]))

    def test_a_face_with_a_hole_has_two_ordered_boundaries(self):
        plate = Part.makeBox(20, 20, 2)
        hole = Part.makeCylinder(4, 5, Vector(10, 10, -1))
        target = 400.0 - 3.141592653589793 * 16.0
        face = next(
            candidate
            for candidate in plate.cut(hole).Faces
            if abs(candidate.Area - target) < 1.0
        )
        unwrapper = self.unwrap(face)
        self.assertEqual(len(unwrapper.getFlatBoundaryNodes()), 2)
        self.assert_loops_are_ordered(unwrapper)


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

    def test_a_null_part_face_is_refused(self):
        if not HAVE_PART:
            self.skipTest("Part is not in this extension build")
        with self.assertRaises(ValueError):
            flatmesh.FaceUnwrapper(Part.Face())

    def test_a_square_stub_unwraps(self):
        unwrapper = flatmesh.FaceUnwrapper(
            _StubFace([(0, 0, 0), (2, 0, 0), (2, 1, 0), (0, 1, 0)], [(0, 1, 2), (0, 2, 3)])
        )
        unwrapper.findFlatNodes(5, 0.95)
        self.assertAlmostEqual(_flat_triangle_area(unwrapper), 2.0, places=9)
        loops = unwrapper.getFlatBoundaryNodes()
        self.assertEqual(len(loops), 1)
        self.assertAlmostEqual(_loop_area(loops[0]), 2.0, places=9)


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
