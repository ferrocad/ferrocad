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
        self.sketch.addGeometry(_segment((0, 0), (2, 2)), False)
        index = self.sketch.addConstraint(
            [
                Sketcher.Constraint("Radius", 0, 5.0),
                Sketcher.Constraint("Equal", 0, 0),
            ]
        )
        self.assertEqual(index, 1, "the last added constraint's index")
        self.assertEqual(self.sketch.ConstraintCount, 2)

        constraints = self.sketch.Constraints
        self.assertEqual([c.Type for c in constraints], ["Radius", "Equal"])
        self.assertAlmostEqual(constraints[0].Value, 5.0)
        self.assertEqual(constraints[1].Second, 0)

    def test_empty_constraint_list_adds_nothing(self):
        self.sketch.addGeometry(_segment((0, 0), (2, 2)), False)
        self.assertEqual(self.sketch.addConstraint([]), -1)
        self.assertEqual(self.sketch.ConstraintCount, 0)

    def test_datum_reads_and_writes_and_refuses_a_non_dimension(self):
        self.sketch.addGeometry(_segment((0, 0), (2, 2)), False)
        self.sketch.addConstraint(
            [
                Sketcher.Constraint("Radius", 0, 5.0),
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
        self.sketch.addGeometry(_segment((0, 0), (2, 2)), False)
        self.sketch.addConstraint(
            [
                Sketcher.Constraint("Radius", 0, 5.0),
                Sketcher.Constraint("Diameter", 0, 9.0),
            ]
        )
        self.sketch.delConstraint(0)
        self.assertEqual(self.sketch.ConstraintCount, 1)
        self.assertEqual(self.sketch.Constraints[0].Type, "Diameter")
        with self.assertRaises(IndexError):
            self.sketch.delConstraint(4)

    def test_solve_reports_a_dangling_reference_but_moves_nothing(self):
        self.sketch.addGeometry(_segment((0, 0), (2, 2)), False)
        self.sketch.addConstraint(Sketcher.Constraint("Vertical", 0))
        self.assertEqual(self.sketch.solve(), 0)
        # A geometry index that does not exist is a model error.
        self.sketch.addConstraint(Sketcher.Constraint("Horizontal", 7))
        self.assertEqual(self.sketch.solve(), -1)

    def test_redundant_constraints_is_empty_without_a_solver(self):
        self.sketch.addGeometry(_segment((0, 0), (2, 2)), False)
        self.sketch.addConstraint(Sketcher.Constraint("Vertical", 0))
        self.assertEqual(self.sketch.RedundantConstraints, [])

    def test_autoconstraints_find_the_coincidence_and_the_axis(self):
        self.sketch.addGeometry(_segment((0, 0), (2, 2)), False)
        self.sketch.addGeometry(_segment((2, 2), (2, 4)), False)
        self.assertEqual(self.sketch.detectMissingPointOnPointConstraints(1e-3), 1)
        self.assertEqual(self.sketch.makeMissingPointOnPointCoincident(False), 1)
        self.assertEqual(self.sketch.detectMissingVerticalHorizontalConstraints(1e-3), 1)
        self.assertEqual(self.sketch.makeMissingVerticalHorizontal(False), 1)

        kinds = [c.Type for c in self.sketch.Constraints]
        self.assertEqual(kinds, ["Coincident", "Vertical"])
        coincident = self.sketch.Constraints[0]
        self.assertEqual(
            (coincident.First, coincident.FirstPos, coincident.Second, coincident.SecondPos),
            (0, Sketcher.PointPos.end, 1, Sketcher.PointPos.start),
        )

        # The pending lists were consumed, so a second pass finds nothing.
        self.assertEqual(self.sketch.detectMissingPointOnPointConstraints(1e-3), 0)

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
