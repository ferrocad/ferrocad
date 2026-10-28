"""The kernel-dependent ``flatmesh`` tests: unwrapping real ``Part`` faces.

Split from ``test_flatmesh`` so the base extension set runs the stub-based tests
and skips nothing: a module is classified as ``workbenches`` in
``contracts/extension-sets.json`` and CI runs it with the workbench modules
provided, where ``Part`` exists.
"""

from __future__ import annotations

import unittest

import flatmesh
from FreeCAD import Vector
from flatmesh_fixtures import boundary_edges, extent, flat_triangle_area, loop_area

import Part


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

        self.assertAlmostEqual(flat_triangle_area(unwrapper), face.Area, places=6)
        loops = unwrapper.getFlatBoundaryNodes()
        self.assertEqual(len(loops), 1)
        self.assertAlmostEqual(loop_area(loops[0]), face.Area, places=6)
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
        areas = sorted(loop_area(loop) for loop in loops)
        self.assertAlmostEqual(areas[0], 3.141592653589793 * 16.0, delta=0.1)
        self.assertAlmostEqual(areas[1], 400.0, places=4)
        # The triangles tile the face with the hole removed.
        self.assertAlmostEqual(flat_triangle_area(unwrapper), face.Area, delta=face.Area * 0.001)

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

        self.assertAlmostEqual(flat_triangle_area(unwrapper), face.Area, delta=face.Area * 0.01)
        loops = unwrapper.getFlatBoundaryNodes()
        self.assertEqual(len(loops), 1)
        self.assertAlmostEqual(loop_area(loops[0]), face.Area, delta=face.Area * 0.01)
        short, long = extent(unwrapper)
        self.assertAlmostEqual(short, 40.0, delta=0.1)
        self.assertAlmostEqual(long, 2 * 3.141592653589793 * 10.0, delta=0.2)

    def test_a_half_cylinder_wall_unwraps_to_its_area(self):
        part = Part.makeCylinder(20, 40, Vector(), Vector(0, 0, 1), 180)
        face = next(
            candidate for candidate in part.Faces if isinstance(candidate.Surface, Part.Cylinder)
        )
        unwrapper = self.unwrap(face)

        self.assertAlmostEqual(flat_triangle_area(unwrapper), face.Area, delta=face.Area * 0.01)
        loops = unwrapper.getFlatBoundaryNodes()
        self.assertEqual(len(loops), 1)
        short, long = extent(unwrapper)
        self.assertAlmostEqual(short, 40.0, delta=0.2)
        self.assertAlmostEqual(long, 3.141592653589793 * 20.0, delta=1.0)

    def test_a_cone_wall_unwraps_to_its_area(self):
        cone = Part.makeCone(10, 6, 20)
        face = cone.Faces[0]
        unwrapper = self.unwrap(face)
        self.assertAlmostEqual(flat_triangle_area(unwrapper), face.Area, delta=face.Area * 0.01)
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
        self.assertEqual(sorted(walked), sorted(boundary_edges(unwrapper)))

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


class NullPartFaceTests(unittest.TestCase):
    def test_a_null_part_face_is_refused(self):
        with self.assertRaises(ValueError):
            flatmesh.FaceUnwrapper(Part.Face())
