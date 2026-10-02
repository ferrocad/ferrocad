"""Tests for the M4 `Base`/`Units`/`Console`/`ParamGet`/`StringHasher` surface.

Exercises the new facade modules exposed from ``FreeCAD`` (over the Rust
``fc-core``/``fc-python`` backend).

Run: PYTHONPATH=python python3 -m unittest tests.test_base_surface -v
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import FreeCAD  # noqa: E402


class TestStringHasherSurface(unittest.TestCase):
    def test_roundtrip_and_type_checks(self):
        h = FreeCAD.StringHasher()
        sid = h.getID("A")
        self.assertGreaterEqual(sid.Value, 1)
        self.assertEqual(sid.Data, "A")
        self.assertTrue(sid.isSame(h.getID("A")))

        with self.assertRaises(TypeError):
            FreeCAD.StringHasher(0)
        with self.assertRaises(ValueError):
            h.getID(0)
        with self.assertRaises(TypeError):
            h.isSame(0)
        with self.assertRaises(TypeError):
            sid.isSame(0)


class TestUnitsAndBase(unittest.TestCase):
    def test_quantity_identity(self):
        self.assertIs(FreeCAD.Base.Quantity, FreeCAD.Units.Quantity)

    def test_quantity_parse(self):
        q = FreeCAD.Base.Quantity("10 mm")
        self.assertEqual(q.value_mm(), 10.0)

    def test_quantity_value_and_unit(self):
        q = FreeCAD.Units.Quantity(1, "m")
        self.assertEqual(q.Value, 1000.0)
        self.assertEqual(q.Unit, FreeCAD.Units.Length)

    def test_quantity_compound(self):
        self.assertEqual(FreeCAD.Units.Quantity("10 m").Value, 10000.0)
        self.assertAlmostEqual(FreeCAD.Units.Quantity("3/8 in").Value, 9.525)
        self.assertEqual(FreeCAD.Units.Quantity("2*pi rad").Value, 360.0)

    def test_get_value_as(self):
        psi = FreeCAD.Units.parseQuantity("1psi")
        self.assertAlmostEqual(psi.getValueAs("MPa").Value, 0.0068947572932, places=8)

    def test_to_number(self):
        self.assertEqual(float(FreeCAD.Units.toNumber(1023, "g", 2)), 1000)
        self.assertEqual(float(FreeCAD.Units.toNumber(1023, "f", 2)), 1023)
        self.assertEqual(float(FreeCAD.Units.toNumber(1023, "e", 2)), 1020)

    def test_schemas(self):
        self.assertEqual(FreeCAD.Units.listSchemas(), ("Standard",))
        psi = FreeCAD.Units.parseQuantity("1psi")
        t = FreeCAD.Units.schemaTranslate(psi, 0)
        self.assertAlmostEqual(
            FreeCAD.Units.parseQuantity(t[0]).getValueAs("psi").Value, 1.0, places=8
        )


class TestConsole(unittest.TestCase):
    def test_print_methods_exist(self):
        for name in ("PrintLog", "PrintMessage", "PrintWarning", "PrintError", "PrintStatus"):
            self.assertTrue(callable(getattr(FreeCAD.Console, name)), name)


class TestParamGet(unittest.TestCase):
    def test_get_and_set(self):
        grp = FreeCAD.ParamGet("User parameter:BaseApp/Preferences/Units")
        self.assertEqual(grp.GetInt("Decimals", 2), 2)
        grp.SetInt("Decimals", 4)
        self.assertEqual(grp.GetInt("Decimals"), 4)
        self.assertEqual(grp.GetBool("Flag", True), True)
        self.assertEqual(grp.GetString("Name", "x"), "x")


class TestGeometry(unittest.TestCase):
    def test_vector(self):
        v = FreeCAD.Vector(1, 2, 3)
        self.assertEqual((v.x, v.y, v.z), (1.0, 2.0, 3.0))
        self.assertEqual(len(v), 3)
        self.assertEqual(v[0], 1.0)
        self.assertEqual(v[-1], 3.0)
        self.assertAlmostEqual(v.Length, (1 + 4 + 9) ** 0.5)
        self.assertEqual(v + FreeCAD.Vector(1, 1, 1), FreeCAD.Vector(2, 3, 4))
        self.assertEqual(v * FreeCAD.Vector(1, 0, 0), 1.0)  # dot product
        self.assertEqual((v * 2.0).x, 2.0)

    def test_placement_identity(self):
        p = FreeCAD.Placement()
        self.assertEqual(p.Base, FreeCAD.Vector(0, 0, 0))

    def test_base_aliases(self):
        self.assertIs(FreeCAD.Vector, FreeCAD.Base.Vector)
        self.assertIs(FreeCAD.Matrix, FreeCAD.Base.Matrix)
        self.assertIs(FreeCAD.Placement, FreeCAD.Base.Placement)

    def test_feature_test_default_properties(self):
        doc = FreeCAD.newDocument("Ft")
        obj = doc.addObject("App::FeatureTest", "F")
        self.assertEqual(obj.Integer, 0)
        self.assertEqual(obj.Float, 0.0)
        self.assertEqual(obj.String, "")
        self.assertIsInstance(obj.Placement, FreeCAD.Placement)
        FreeCAD.closeDocument("Ft")


if __name__ == "__main__":
    unittest.main()
