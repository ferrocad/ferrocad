"""The ``Sketcher`` constraint surface: the model, its encoding and its limits.

``Sketcher.Constraint`` is a record, not an operation: ``Sketcher::SketchObject``
stores the records it is given, decodes them back into objects, and reports
whether every reference resolves. No numerical solve happens (that is P4 SK2's
PlanGCS binding), so these tests pin the model and the honest gaps rather than
any geometry movement.

Run::

    python3 tools/run_tests.py --extension workbenches
"""

from __future__ import annotations

import tempfile
import unittest

import FreeCAD as App
import Part
import Sketcher


def _document(name):
    document = App.newDocument(name)
    sketch = document.addObject("Sketcher::SketchObject", "Sketch")
    return document, sketch


def _close(document):
    App.closeDocument(document.Name)


def _segment(start, end):
    return Part.LineSegment(App.Vector(*start), App.Vector(*end))


class ConstraintRecordTests(unittest.TestCase):
    def test_a_dimensional_constraint_carries_its_datum(self):
        constraint = Sketcher.Constraint("Radius", 2, 12.5)
        self.assertEqual(constraint.Type, "Radius")
        self.assertEqual(constraint.First, 2)
        self.assertEqual(constraint.Value, 12.5)
        self.assertTrue(constraint.Driving)
        self.assertTrue(constraint.IsActive)

    def test_two_geometry_ids_name_first_and_second(self):
        constraint = Sketcher.Constraint("Equal", 0, 1)
        self.assertEqual((constraint.First, constraint.Second), (0, 1))

    def test_a_point_datum_names_a_point_position(self):
        constraint = Sketcher.Constraint("Distance", 0, Sketcher.PointPos.end, 5.0)
        self.assertEqual(constraint.First, 0)
        self.assertEqual(constraint.FirstPos, Sketcher.PointPos.end)
        self.assertEqual(constraint.Value, 5.0)

    def test_two_curves_with_a_value_keep_both_geometries(self):
        constraint = Sketcher.Constraint("Angle", 0, 1, 45.0)
        self.assertEqual((constraint.First, constraint.Second), (0, 1))
        self.assertEqual(constraint.Value, 45.0)

    def test_four_ids_are_two_points(self):
        constraint = Sketcher.Constraint(
            "Coincident", 0, Sketcher.PointPos.end, 1, Sketcher.PointPos.start
        )
        self.assertEqual(
            (
                constraint.First,
                constraint.FirstPos,
                constraint.Second,
                constraint.SecondPos,
            ),
            (0, 2, 1, 1),
        )

    def test_keywords_override_the_positional_form(self):
        constraint = Sketcher.Constraint("Radius", 0, 1.0, Value=9.0, Name="r1")
        self.assertEqual(constraint.Value, 9.0)
        self.assertEqual(constraint.Name, "r1")

    def test_a_constraint_label_position_is_refused_by_name(self):
        with self.assertRaises(TypeError) as caught:
            Sketcher.Constraint("Distance", 0, 5.0, LabelPosition=1)
        self.assertIn("LabelPosition", str(caught.exception))

    def test_an_unknown_keyword_is_refused(self):
        with self.assertRaises(TypeError):
            Sketcher.Constraint("Radius", 0, 1.0, Colour="red")

    def test_the_type_must_be_a_string(self):
        with self.assertRaises(TypeError):
            Sketcher.Constraint(3, 0)

    def test_the_solver_is_not_part_of_this_module(self):
        with self.assertRaises(AttributeError) as caught:
            Sketcher.Solver  # noqa: B018 - attribute access is the assertion
        self.assertIn("Constraint", str(caught.exception))


class SketchDocumentTests(unittest.TestCase):
    def setUp(self):
        self.document, self.sketch = _document("SketchConstraints")

    def tearDown(self):
        _close(self.document)

    def test_geometry_count_tracks_add_geometry(self):
        self.assertEqual(self.sketch.GeometryCount, 0)
        self.sketch.addGeometry(_segment((0, 0), (2, 2)), False)
        self.sketch.addGeometry(_segment((2, 2), (2, 4)), False)
        self.assertEqual(self.sketch.GeometryCount, 2)

    def test_constraints_round_trip_through_the_document(self):
        self.sketch.addGeometry(_segment((0, 0), (2, 0)), False)
        index = self.sketch.addConstraint(
            [
                Sketcher.Constraint("Distance", 0, 1, 0, 2, 5.0),
                Sketcher.Constraint("Equal", 0, 0),
            ]
        )
        self.assertEqual(index, 1, "the last added constraint's index")
        self.assertEqual(self.sketch.ConstraintCount, 2)

        constraints = self.sketch.Constraints
        self.assertEqual([c.Type for c in constraints], ["Distance", "Equal"])
        self.assertAlmostEqual(constraints[0].Value, 5.0)
        self.assertEqual(constraints[1].Second, 0)

    def test_empty_constraint_list_adds_nothing(self):
        self.sketch.addGeometry(_segment((0, 0), (2, 2)), False)
        self.assertEqual(self.sketch.addConstraint([]), -1)
        self.assertEqual(self.sketch.ConstraintCount, 0)

    def test_datum_reads_and_writes_and_refuses_a_non_dimension(self):
        self.sketch.addGeometry(_segment((0, 0), (2, 0)), False)
        self.sketch.addConstraint(
            [
                Sketcher.Constraint("Distance", 0, 1, 0, 2, 5.0),
                Sketcher.Constraint("Vertical", 0),
            ]
        )
        self.assertAlmostEqual(self.sketch.getDatum(0), 5.0)
        self.sketch.setDatum(0, 7.5)
        self.assertAlmostEqual(self.sketch.getDatum(0), 7.5)
        with self.assertRaises(ValueError):
            self.sketch.getDatum(1)
        with self.assertRaises(ValueError):
            self.sketch.setDatum(1, 3.0)

    def test_deleting_a_constraint_reindexes_the_rest(self):
        self.sketch.addGeometry(_segment((0, 0), (2, 0)), False)
        self.sketch.addConstraint(
            [
                Sketcher.Constraint("Distance", 0, 1, 0, 2, 5.0),
                Sketcher.Constraint("DistanceX", 0, 1, 0, 2, 9.0),
            ]
        )
        self.sketch.delConstraint(0)
        self.assertEqual(self.sketch.ConstraintCount, 1)
        self.assertEqual(self.sketch.Constraints[0].Type, "DistanceX")
        with self.assertRaises(IndexError):
            self.sketch.delConstraint(4)

    def test_unused_references_are_geoundef_and_roots_are_negative(self):
        vertical = Sketcher.Constraint("Vertical", 0)
        self.assertEqual(vertical.Second, Sketcher.GeoUndef)
        self.assertEqual(vertical.Third, Sketcher.GeoUndef)
        self.assertEqual(Sketcher.GeoUndef, -2000)

        root = Sketcher.Constraint(
            "Coincident", -1, Sketcher.PointPos.start, 0, Sketcher.PointPos.start
        )
        self.assertEqual(root.First, -1)
        self.assertEqual(root.Second, 0)

    def test_solve_reports_a_dangling_record_but_moves_nothing(self):
        self.sketch.addGeometry(_segment((0, 0), (2, 0)), False)
        self.sketch.addConstraint(Sketcher.Constraint("Vertical", 0))
        # Four parameters, one row: the line may still translate.
        self.assertEqual(self.sketch.solve(), 3)

        # An index that names no geometry is refused while the record is read.
        with self.assertRaises(IndexError):
            self.sketch.addConstraint(Sketcher.Constraint("Horizontal", 7))

        # A record already in the property that names a vanished id is a model
        # error: `solve` reports it and the getter refuses to invent a position.
        self.sketch.ConstraintData = ["Vertical;first=#gone"]
        self.assertEqual(self.sketch.solve(), -1)
        with self.assertRaises(ValueError) as caught:
            self.sketch.Constraints  # noqa: B018 - the getter is the assertion
        self.assertIn("gone", str(caught.exception))

    def test_redundant_constraints_is_empty_without_a_solver(self):
        self.sketch.addGeometry(_segment((0, 0), (2, 2)), False)
        self.sketch.addConstraint(Sketcher.Constraint("Vertical", 0))
        self.assertEqual(self.sketch.RedundantConstraints, [])

    def test_autoconstraints_find_the_pair_the_axis_and_the_root(self):
        self.sketch.addGeometry(_segment((0, 0), (2, 2)), False)
        self.sketch.addGeometry(_segment((2, 2), (2, 4)), False)
        # The pair, plus the first edge's start on the origin (the root point).
        self.assertEqual(self.sketch.detectMissingPointOnPointConstraints(1e-3), 2)
        self.assertEqual(self.sketch.makeMissingPointOnPointCoincident(False), 2)
        self.assertEqual(self.sketch.detectMissingVerticalHorizontalConstraints(1e-3), 1)
        self.assertEqual(self.sketch.makeMissingVerticalHorizontal(False), 1)

        constraints = self.sketch.Constraints
        self.assertEqual([c.Type for c in constraints], ["Coincident", "Coincident", "Vertical"])
        pair = constraints[0]
        self.assertEqual(
            (pair.First, pair.FirstPos, pair.Second, pair.SecondPos),
            (0, Sketcher.PointPos.end, 1, Sketcher.PointPos.start),
        )
        root = constraints[1]
        self.assertEqual(
            (root.First, root.FirstPos, root.Second, root.SecondPos),
            (-1, Sketcher.PointPos.start, 0, Sketcher.PointPos.start),
            "an endpoint on the origin is a coincidence with the root point",
        )

        # The pending lists were consumed, so a second pass finds nothing.
        self.assertEqual(self.sketch.detectMissingPointOnPointConstraints(1e-3), 0)

    def test_deleting_geometry_keeps_the_surviving_constraints(self):
        for start, end in [((0, 0), (1, 0)), ((1, 0), (2, 0)), ((2, 0), (3, 0))]:
            self.sketch.addGeometry(_segment(start, end), False)
        self.assertEqual(list(self.sketch.GeometryIds), ["g1", "g2", "g3"])

        self.sketch.addConstraint(Sketcher.Constraint("Equal", 0, 2))
        self.sketch.addConstraint(Sketcher.Constraint("Equal", 1, 2))
        self.sketch.delGeometry(1)

        self.assertEqual(self.sketch.GeometryCount, 2)
        self.assertEqual(list(self.sketch.GeometryIds), ["g1", "g3"])
        self.assertEqual(self.sketch.ConstraintCount, 1)
        survivor = self.sketch.Constraints[0]
        self.assertEqual(
            (survivor.First, survivor.Second),
            (0, 1),
            "the surviving constraint is re-indexed, not scrambled",
        )
        self.assertEqual(
            self.sketch.solve(),
            7,
            "two lines (8 parameters) and one length equality",
        )
        with self.assertRaises(IndexError):
            self.sketch.delGeometry(5)

    def test_construction_geometry_is_addressable_but_not_drawn(self):
        self.sketch.addGeometry(_segment((0, 0), (1, 0)), False)
        self.sketch.addGeometry(_segment((0, 1), (0, 2)), True)
        self.document.recompute()

        self.assertEqual(self.sketch.GeometryCount, 2)
        self.assertFalse(self.sketch.getConstruction(0))
        self.assertTrue(self.sketch.getConstruction(1))
        self.assertEqual(len(self.sketch.Shape.Edges), 1, "only the visible edge is drawn")

        # A construction element is a valid constraint target.
        self.sketch.addConstraint(Sketcher.Constraint("Vertical", 1))
        self.assertEqual(self.sketch.solve(), 7)

        self.sketch.toggleConstruction(1)
        self.document.recompute()
        self.assertFalse(self.sketch.getConstruction(1))
        self.assertEqual(len(self.sketch.Shape.Edges), 2)
        self.assertEqual(list(self.sketch.GeometryIds), ["g1", "g2"], "toggling keeps ids")
        self.assertEqual(self.sketch.ConstraintCount, 1)

        with self.assertRaises(IndexError):
            self.sketch.getConstruction(9)
        with self.assertRaises(IndexError):
            self.sketch.toggleConstruction(9)

    def test_a_sketch_without_the_identity_properties_is_migrated(self):
        self.sketch.addGeometry(_segment((0, 0), (1, 0)), False)
        # Drop the pair the way a document written before the identity would
        # lack them, then make the sketch recreate them.
        self.sketch.removeProperty("GeometryIds")
        self.sketch.removeProperty("Construction")
        self.sketch.addGeometry(_segment((0, 1), (0, 2)), True)

        self.assertEqual(list(self.sketch.GeometryIds), ["g1", "g2"])
        self.assertTrue(self.sketch.getConstruction(1))
        self.assertEqual(self.sketch.solve(), 8, "two unconstrained lines")

    def test_root_axis_references_are_valid_and_external_ones_are_not(self):
        self.sketch.addGeometry(_segment((0, 0), (1, 0)), False)
        self.sketch.addConstraint(
            Sketcher.Constraint(
                "Coincident", -1, Sketcher.PointPos.start, 0, Sketcher.PointPos.start
            )
        )
        self.assertEqual(self.sketch.solve(), 2, "four parameters, two rows")
        self.assertEqual(self.sketch.ConstraintData, [
            "Coincident;first=-1;firstPos=1;second=#g1;secondPos=1"
        ])
        self.assertEqual(self.sketch.Constraints[0].First, -1)

        with self.assertRaises(ValueError) as caught:
            self.sketch.addConstraint(Sketcher.Constraint("Vertical", -3))
        self.assertIn("external geometry", str(caught.exception))

    def test_a_constraint_needs_a_geometric_reference(self):
        with self.assertRaises(ValueError) as caught:
            self.sketch.addConstraint(Sketcher.Constraint("Vertical", Sketcher.GeoUndef))
        self.assertIn("names no geometry", str(caught.exception))

    def test_an_unknown_constraint_type_is_refused_with_the_list(self):
        class _Record:
            Type = "SnellsLaw"
            First = 0

        with self.assertRaises(ValueError) as caught:
            self.sketch.addConstraint(_Record())
        message = str(caught.exception)
        self.assertIn("SnellsLaw", message)
        self.assertIn("Coincident", message)

    def test_the_surface_belongs_to_sketches_only(self):
        box = self.document.addObject("Part::Box", "Box")
        for name in ("addConstraint", "solve", "ConstraintCount"):
            with self.assertRaises(AttributeError):
                getattr(box, name)


class SketchPersistenceTests(unittest.TestCase):
    """The identity, the flags and the constraints are ordinary properties."""

    def test_identity_construction_and_constraints_survive_a_reload(self):
        document, sketch = _document("SketchPersistence")
        reopened = None
        with tempfile.TemporaryDirectory() as tmp:
            path = f"{tmp}/sketch.FCStd"
            try:
                sketch.addGeometry(_segment((0, 0), (1, 0)), False)
                sketch.addGeometry(_segment((0, 1), (0, 2)), True)
                sketch.addConstraint(Sketcher.Constraint("Equal", 0, 1))
                sketch.addConstraint(
                    Sketcher.Constraint(
                        "Coincident", -1, Sketcher.PointPos.start, 0, Sketcher.PointPos.start
                    )
                )
                document.saveAs(path)

                reopened = App.openDocument(path)
                loaded = reopened.getObject("Sketch")
                self.assertEqual(list(loaded.GeometryIds), ["g1", "g2"])
                self.assertEqual(list(loaded.Construction), [False, True])
                self.assertEqual(loaded.solve(), 5, "8 parameters, 3 constraint rows")
                constraints = loaded.Constraints
                self.assertEqual([c.Type for c in constraints], ["Equal", "Coincident"])
                self.assertEqual(constraints[1].First, -1, "a root reference survives")
            finally:
                App.closeDocument(document.Name)
                if reopened is not None:
                    App.closeDocument(reopened.Name)


class SketchSolverTests(unittest.TestCase):
    """The constraints actually move geometry.

    `solve()` returns the remaining degrees of freedom (upstream's convention: `0`
    means fully constrained) and `-1` means "not solved" — a dangling reference, a
    constraint the solver cannot apply, or a system that does not converge.
    """

    def setUp(self):
        self.document, self.sketch = _document("SketchSolver")

    def tearDown(self):
        _close(self.document)

    def _rectangle(self):
        """Four roughly, not exactly, rectangular sides."""
        for start, end in [
            ((0.0, 0.0), (10.3, 0.2)),
            ((10.0, 0.4), (9.8, 6.0)),
            ((9.9, 6.3), (0.1, 6.1)),
            ((0.2, 5.8), (0.0, 0.3)),
        ]:
            self.sketch.addGeometry(_segment(start, end), False)
        self.sketch.addConstraint(
            [
                Sketcher.Constraint("Coincident", 0, 2, 1, 1),
                Sketcher.Constraint("Coincident", 1, 2, 2, 1),
                Sketcher.Constraint("Coincident", 2, 2, 3, 1),
                Sketcher.Constraint("Coincident", 3, 2, 0, 1),
                Sketcher.Constraint("Horizontal", 0),
                Sketcher.Constraint("Vertical", 1),
                Sketcher.Constraint("Horizontal", 2),
                Sketcher.Constraint("Vertical", 3),
            ]
        )

    def test_a_perturbed_rectangle_closes_and_keeps_four_degrees_of_freedom(self):
        self._rectangle()
        self.assertEqual(
            self.sketch.solve(), 4, "width, height, x and y are still free"
        )
        self.document.recompute()

        edges = self.sketch.Shape.Edges
        self.assertEqual(len(edges), 4)
        points = [(edge.Vertexes[0].Point, edge.Vertexes[-1].Point) for edge in edges]
        for index in range(4):
            self.assertTrue(
                points[index][1].isEqual(points[(index + 1) % 4][0], 1e-6),
                f"corner {index} is open",
            )
        # Axis alignment: the horizontal sides share a Y, the vertical ones an X.
        self.assertAlmostEqual(points[0][0].y, points[0][1].y, places=6)
        self.assertAlmostEqual(points[1][0].x, points[1][1].x, places=6)

    def test_a_distance_datum_resizes_the_profile(self):
        self._rectangle()
        self.assertGreaterEqual(self.sketch.solve(), 0)
        self.sketch.addConstraint(
            Sketcher.Constraint("DistanceX", 0, 1, 1, 1, 20.0)
        )
        self.sketch.addConstraint(
            Sketcher.Constraint("DistanceY", 1, 1, 1, 2, 8.0)
        )
        self.assertEqual(self.sketch.solve(), 2, "only the origin is free")
        self.document.recompute()

        box = self.sketch.Shape.BoundBox
        self.assertAlmostEqual(box.XLength, 20.0, places=6)
        self.assertAlmostEqual(box.YLength, 8.0, places=6)

    def test_a_radius_datum_resizes_a_circle(self):
        self.sketch.addGeometry(
            Part.Circle(App.Vector(0, 0, 0), App.Vector(0, 0, 1), 3.0), False
        )
        self.assertEqual(self.sketch.solve(), 3, "centre and radius are free")
        self.sketch.addConstraint(Sketcher.Constraint("Radius", 0, 7.5))
        self.assertEqual(self.sketch.solve(), 2)
        self.document.recompute()
        self.assertAlmostEqual(self.sketch.Shape.Edges[0].Curve.Radius, 7.5, places=6)

    def test_a_construction_line_drives_the_real_profile(self):
        self.sketch.addGeometry(_segment((0, 0), (4, 0)), False)
        self.sketch.addGeometry(_segment((0, 5), (1, 5)), True)
        self.sketch.addConstraint(Sketcher.Constraint("Equal", 0, 1))
        self.sketch.addConstraint(
            Sketcher.Constraint("Distance", 1, 1, 1, 2, 7.0)
        )
        self.assertGreaterEqual(self.sketch.solve(), 0)
        self.document.recompute()

        edges = self.sketch.Shape.Edges
        self.assertEqual(len(edges), 1, "construction geometry is not drawn")
        self.assertAlmostEqual(edges[0].Length, 7.0, places=6)

    def test_a_duplicate_constraint_is_reported_one_based(self):
        self.sketch.addGeometry(_segment((0, 0), (4, 0)), False)
        self.sketch.addConstraint(Sketcher.Constraint("Vertical", 0))
        self.sketch.addConstraint(Sketcher.Constraint("Vertical", 0))
        self.assertGreaterEqual(self.sketch.solve(), 0)
        self.assertEqual(
            self.sketch.RedundantConstraints,
            [2],
            "the later duplicate, 1-based as upstream reports it",
        )
        self.assertEqual(self.sketch.ConflictingConstraints, [])

    def test_contradictory_constraints_are_refused_and_named(self):
        self.sketch.addGeometry(_segment((0, 0), (4, 0)), False)
        self.sketch.addConstraint(Sketcher.Constraint("Horizontal", 0))
        self.sketch.addConstraint(Sketcher.Constraint("Distance", 0, 1, 0, 2, 10.0))
        self.sketch.addConstraint(Sketcher.Constraint("Distance", 0, 1, 0, 2, 20.0))

        self.assertEqual(self.sketch.solve(), -1, "the two datums cannot both hold")
        conflicting = self.sketch.ConflictingConstraints
        self.assertIn(3, conflicting, "the second distance is named")
        # A refused solve does not touch the geometry.
        self.document.recompute()
        self.assertAlmostEqual(self.sketch.Shape.Edges[0].Length, 4.0, places=6)

    def test_a_constraint_the_solver_cannot_apply_is_refused(self):
        self.sketch.addGeometry(_segment((0, 0), (4, 0)), False)
        with self.assertRaises(ValueError) as caught:
            self.sketch.addConstraint(Sketcher.Constraint("Radius", 0, 3.0))
        self.assertIn("radius", str(caught.exception))
        self.assertEqual(self.sketch.ConstraintCount, 0)


class SketchPlaneTests(unittest.TestCase):
    """`Draft.make_sketch` asks a wire for its plane; Part answers for it."""

    def test_a_bent_wire_has_a_plane(self):
        wire = Part.Wire(
            [
                _segment((0, 0), (2, 2)).toShape(),
                _segment((2, 2), (2, 4)).toShape(),
            ]
        )
        plane = wire.findPlane(1e-3)
        self.assertIsNotNone(plane)
        self.assertAlmostEqual(abs(plane.Axis.z), 1.0, places=9)

    def test_a_straight_edge_has_no_plane(self):
        self.assertIsNone(_segment((0, 0), (2, 2)).toShape().findPlane(1e-3))


if __name__ == "__main__":
    unittest.main()
