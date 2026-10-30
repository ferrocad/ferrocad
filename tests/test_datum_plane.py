"""The datum-plane entry point and its workplane command.

`App_SaveDatumPlane` is shell-owned: the active workplane is transient viewport
state, so the Rust shell reads the live frame and calls
``ferrocad_parametric.add_datum_plane``. These tests pin the script half — the
``Part::DatumPlane`` object it creates from the frame, its derived ``Face1``
shape, the JSON reply, the single undo step, and the command's place in the
workplane surface.

The face must be pickable and bindable: ``Set WP`` binds a planar face by its
element name, so the tessellation has to name it ``Face1``.

``Part`` is required; run with ``tools/run_tests.py --extension Part`` (or the
workbenches preset), not ``unittest discover`` in a base-only checkout.
"""

from __future__ import annotations

import json
import math
import os
import tempfile
import unittest

import FreeCAD
import FreeCADGui
import ferrocad_parametric
import ferrocad_shell

ORIGIN = (10.0, 20.0, 30.0)
U = (1.0, 0.0, 0.0)
# Tilted so a frame mix-up cannot pass as the world XY plane.
V = (0.0, 0.6, 0.8)


def payload(origin=ORIGIN, u=U, v=V):
    return json.dumps({"origin": list(origin), "u": list(u), "v": list(v)})


def cross(u, v):
    return (
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    )


class DatumPlaneTests(unittest.TestCase):
    def setUp(self):
        self.doc = FreeCAD.newDocument("DatumPlaneTests")

    def tearDown(self):
        # A round-trip test closes `self.doc` and reopens it; only close a live one.
        if FreeCAD.getDocument(self.doc.Name) is not None:
            FreeCAD.closeDocument(self.doc.Name)

    def add(self, **frame):
        return json.loads(ferrocad_parametric.add_datum_plane(payload(**frame)))

    def test_the_face_is_created_as_one_undoable_object(self):
        reply = self.add()
        self.assertTrue(reply["ok"], reply)
        name = reply["object"]

        self.assertEqual([obj.Name for obj in self.doc.Objects], [name])
        self.assertEqual(self.doc.getAvailableUndos(), 1)

        self.assertTrue(self.doc.undo())
        self.assertIsNone(self.doc.getObject(name))
        self.assertEqual(self.doc.getAvailableUndos(), 0)

    def test_the_object_is_a_part_datum_plane(self):
        name = self.add()["object"]
        obj = self.doc.getObject(name)

        self.assertEqual(obj.TypeId, "Part::DatumPlane")
        self.assertIn("Placement", obj.PropertiesList)
        self.assertIn("Size", obj.PropertiesList)
        self.assertIn("Shape", obj.PropertiesList)
        self.assertEqual(obj.getTypeIdOfProperty("Size"), "App::PropertyLength")
        self.assertAlmostEqual(obj.Size.Value, 100.0, delta=1e-9)
        # The frame is the placement: origin plus a rotation whose local Z is `n`.
        for actual, wanted in zip(
            (obj.Placement.Base.x, obj.Placement.Base.y, obj.Placement.Base.z), ORIGIN
        ):
            self.assertAlmostEqual(actual, wanted, delta=1e-9)
        local_z = obj.Placement.Rotation.multVec(FreeCAD.Vector(0.0, 0.0, 1.0))
        for actual, wanted in zip((local_z.x, local_z.y, local_z.z), cross(U, V)):
            self.assertAlmostEqual(actual, wanted, delta=1e-9)

    def test_the_face_is_planar_and_follows_the_frame(self):
        name = self.add()["object"]
        shape = self.doc.getObject(name).Shape

        self.assertTrue(shape.isValid())
        self.assertEqual(shape.ShapeType, "Face")
        # A face built from a planar wire reports a plane surface, and `normalAt`
        # refuses a non-planar one.
        self.assertEqual(shape.Surface.TypeId, "Part::GeomPlane")
        self.assertAlmostEqual(shape.Area, 10000.0, delta=1.0)

        normal = shape.normalAt(0.0, 0.0)
        for actual, wanted in zip((normal.x, normal.y, normal.z), cross(U, V)):
            self.assertAlmostEqual(actual, wanted, delta=1e-9)

    def test_the_shape_offers_one_pickable_face(self):
        name = self.add()["object"]
        parts = list(self.doc.getObject(name).Shape.mesh()["parts"])

        # The kernel's part list enumerates the face, then its four edges and four
        # vertices. Exactly one entry is a face, and that name is what `Set WP`
        # binds.
        self.assertEqual(parts[0], "Face1")
        self.assertEqual(
            [part for part in parts if part.startswith("Face")], ["Face1"]
        )

    def test_a_degenerate_frame_is_rejected_without_an_object(self):
        frames = (
            {"u": (0.0, 0.0, 0.0)},
            {"v": (0.0, 0.0, 0.0)},
            # Parallel to the default `u = (1, 0, 0)`.
            {"v": (2.0, 0.0, 0.0)},
            # Not finite, so no plane can be built.
            {"u": (math.nan, 0.0, 0.0)},
            {"u": (math.inf, 0.0, 0.0)},
        )
        for frame in frames:
            with self.subTest(frame=frame):
                reply = self.add(**frame)
                self.assertFalse(reply["ok"])
                self.assertTrue(reply["error"])
                self.assertEqual(list(self.doc.Objects), [])
                self.assertEqual(self.doc.getAvailableUndos(), 0)

    def test_placement_and_size_survive_an_fcstd_round_trip(self):
        name = self.add()["object"]

        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "datum.FCStd")
            self.doc.saveAs(path)
            FreeCAD.closeDocument(self.doc.Name)
            reopened = FreeCAD.openDocument(path)
            try:
                obj = reopened.getObject(name)
                self.assertIsNotNone(obj, "the datum plane is restored")
                self.assertEqual(obj.TypeId, "Part::DatumPlane")
                for actual, wanted in zip(
                    (obj.Placement.Base.x, obj.Placement.Base.y, obj.Placement.Base.z),
                    ORIGIN,
                ):
                    self.assertAlmostEqual(actual, wanted, delta=1e-9)
                self.assertAlmostEqual(obj.Size.Value, 100.0, delta=1e-9)
                self.assertAlmostEqual(obj.Shape.Area, 10000.0, delta=1.0)
                self.assertEqual(obj.Shape.mesh()["parts"][0], "Face1")
            finally:
                FreeCAD.closeDocument(reopened.Name)

    def test_a_size_edit_rebuilds_the_face(self):
        name = self.add()["object"]
        obj = self.doc.getObject(name)

        self.doc.openTransaction("Set size")
        obj.Size = "40 mm"
        self.doc.commitTransaction()
        self.doc.recompute()

        self.assertAlmostEqual(obj.Shape.Area, 1600.0, delta=1.0)
        self.assertEqual(self.doc.getAvailableUndos(), 2, "creation + the edit")

    def test_a_malformed_payload_is_rejected_without_an_object(self):
        truncated = json.dumps({"origin": [0.0, 0.0], "u": U, "v": V})
        for raw in ("{not json", truncated):
            with self.subTest(payload=raw):
                reply = json.loads(ferrocad_parametric.add_datum_plane(raw))
                self.assertFalse(reply["ok"])
                self.assertTrue(reply["error"])
                self.assertEqual(list(self.doc.Objects), [])


class SaveDatumPlaneCommandTests(unittest.TestCase):
    def test_the_command_lives_on_the_workplane_surface(self):
        entries = {
            entry["id"]: entry for entry in json.loads(ferrocad_shell.commands())
        }
        entry = entries["App_SaveDatumPlane"]

        self.assertEqual(entry["surface"], "workplane")
        self.assertEqual(entry["parent"], "App_Workplane")
        self.assertEqual(entry["short"], "Save WP")
        self.assertTrue(entry["wheel"])
        self.assertEqual(entry["tone"], "additive")

    def test_running_it_headless_reports_the_shell_owns_it(self):
        # No viewport here, so there is no live frame to save.
        self.assertEqual(
            FreeCADGui.runCommand("App_SaveDatumPlane"),
            {"status": "save datum plane: the shell owns this command"},
        )


if __name__ == "__main__":
    unittest.main()
