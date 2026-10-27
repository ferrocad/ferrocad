"""App-facade coverage for `Base.Vector2d` arithmetic and `App.Material`.

SimpleCAD's surface wrapper subtracts two `Vector2d` values, and its appearance
code copies all six upstream material fields. These tests pin the operator set
against `Vector`'s conventions in the same binding, and the upstream
`Material()` defaults, plus the write-back path through `ViewObject`.
"""

import math
import unittest

import FreeCAD


class Vector2dTests(unittest.TestCase):
    def test_constructors(self):
        self.assertEqual(FreeCAD.Vector2d(), FreeCAD.Vector2d(0.0, 0.0))
        self.assertEqual(FreeCAD.Vector2d(1.0, 2.0), FreeCAD.Vector2d((1.0, 2.0)))
        source = FreeCAD.Vector2d(3.0, 4.0)
        self.assertEqual(FreeCAD.Vector2d(source), source)
        with self.assertRaises(ValueError):
            FreeCAD.Vector2d((1.0, 2.0, 3.0))

    def test_add_sub_and_negative(self):
        a = FreeCAD.Vector2d(1.0, 2.0)
        b = FreeCAD.Vector2d(3.0, 5.0)
        self.assertEqual(a + b, FreeCAD.Vector2d(4.0, 7.0))
        # The exact operation SimpleCAD's wrap.py:646 performs.
        self.assertEqual(b - a, FreeCAD.Vector2d(2.0, 3.0))
        self.assertEqual(-a, FreeCAD.Vector2d(-1.0, -2.0))
        self.assertEqual(a.add(b), FreeCAD.Vector2d(4.0, 7.0))
        self.assertEqual(a.sub(b), FreeCAD.Vector2d(-2.0, -3.0))
        self.assertEqual(a.negative(), FreeCAD.Vector2d(-1.0, -2.0))
        # The operands are untouched.
        self.assertEqual(a, FreeCAD.Vector2d(1.0, 2.0))

    def test_scalar_and_dot_product(self):
        a = FreeCAD.Vector2d(1.0, 2.0)
        self.assertEqual(a * 2.0, FreeCAD.Vector2d(2.0, 4.0))
        self.assertEqual(2.0 * a, FreeCAD.Vector2d(2.0, 4.0))
        self.assertEqual(a * FreeCAD.Vector2d(3.0, 4.0), 11.0)
        self.assertEqual(a.dot(FreeCAD.Vector2d(3.0, 4.0)), 11.0)
        self.assertEqual(a / 2.0, FreeCAD.Vector2d(0.5, 1.0))
        with self.assertRaises(ZeroDivisionError):
            a / 0.0
        with self.assertRaises(TypeError):
            a * "two"

    def test_cross_product_is_the_z_component(self):
        self.assertEqual(
            FreeCAD.Vector2d(1.0, 0.0).cross(FreeCAD.Vector2d(0.0, 1.0)), 1.0
        )

    def test_length_abs_and_distance(self):
        a = FreeCAD.Vector2d(3.0, 4.0)
        self.assertEqual(a.Length, 5.0)
        self.assertEqual(abs(a), 5.0)
        self.assertEqual(a.distanceToPoint(FreeCAD.Vector2d(0.0, 0.0)), 5.0)
        self.assertEqual(a.distanceToPoint(FreeCAD.Vector2d(3.0, 9.0)), 5.0)
        self.assertTrue(a.isEqual(FreeCAD.Vector2d(3.0, 4.0 + 1e-13), 1e-12))
        self.assertFalse(a.isEqual(FreeCAD.Vector2d(3.0, 4.1), 1e-12))

        a.Length = 10.0
        self.assertAlmostEqual(a.Length, 10.0)
        self.assertAlmostEqual(a.x, 6.0)
        self.assertAlmostEqual(a.y, 8.0)

    def test_normalise_in_place_and_copy(self):
        a = FreeCAD.Vector2d(3.0, 4.0)
        copy = a.normalized()
        self.assertAlmostEqual(copy.x, 0.6)
        self.assertAlmostEqual(copy.y, 0.8)
        self.assertAlmostEqual(copy.Length, 1.0)
        # `normalize()` mutates and returns self, like `Vector.normalize()`.
        self.assertEqual(a, FreeCAD.Vector2d(3.0, 4.0))
        returned = a.normalize()
        self.assertEqual(returned, copy)
        self.assertAlmostEqual(a.Length, 1.0)
        # The null vector stays null rather than producing NaNs.
        zero = FreeCAD.Vector2d().normalize()
        self.assertEqual(zero, FreeCAD.Vector2d())

    def test_rotate_returns_self(self):
        v = FreeCAD.Vector2d(1.0, 1.0)
        returned = v.rotate(math.pi / 2)
        self.assertAlmostEqual(v.x, -1.0)
        self.assertAlmostEqual(v.y, 1.0)
        self.assertEqual(returned, v)

    def test_angles_and_line_distance(self):
        x_axis = FreeCAD.Vector2d(1.0, 0.0)
        y_axis = FreeCAD.Vector2d(0.0, 1.0)
        self.assertAlmostEqual(x_axis.getAngle(y_axis), math.pi / 2)
        self.assertAlmostEqual(
            FreeCAD.Vector2d(1.0, 1.0).distanceToLine(
                FreeCAD.Vector2d(0.0, 0.0), FreeCAD.Vector2d(1.0, 0.0)
            ),
            1.0,
        )

    def test_comparisons_and_sequence_protocol(self):
        a = FreeCAD.Vector2d(1.0, 2.0)
        self.assertTrue(a == FreeCAD.Vector2d(1.0, 2.0))
        self.assertTrue(a != FreeCAD.Vector2d(1.0, 2.0001))
        self.assertNotEqual(a, (1.0, 2.0))
        self.assertEqual(len(a), 2)
        self.assertEqual(a[0], 1.0)
        self.assertEqual(a[-1], 2.0)
        a[0] = 7.0
        self.assertEqual(a.x, 7.0)
        with self.assertRaises(IndexError):
            a[2]

    def test_repr(self):
        self.assertEqual(repr(FreeCAD.Vector2d(1.0, 2.0)), "Vector2d (1, 2)")


class MaterialTests(unittest.TestCase):
    # Upstream `App::Material()` is `setType(DEFAULT)` in Material.cpp.
    DEFAULTS = {
        "DiffuseColor": (0.8, 0.8, 0.9, 1.0),
        "AmbientColor": (0.3333, 0.3333, 0.3333, 1.0),
        "SpecularColor": (0.5333, 0.5333, 0.5333, 1.0),
        "EmissiveColor": (0.0, 0.0, 0.0, 1.0),
    }

    def test_defaults_match_upstream(self):
        material = FreeCAD.Material()
        for name, expected in self.DEFAULTS.items():
            actual = getattr(material, name)
            self.assertEqual(len(actual), 4)
            for got, want in zip(actual, expected):
                self.assertAlmostEqual(got, want, places=6, msg=name)
        self.assertAlmostEqual(material.Shininess, 0.9)
        self.assertEqual(material.Transparency, 0.0)
        self.assertEqual(repr(material), "<Material object>")

    def test_every_colour_round_trips(self):
        material = FreeCAD.Material()
        for name in ("DiffuseColor", "AmbientColor", "SpecularColor", "EmissiveColor"):
            setattr(material, name, (0.1, 0.2, 0.3))
            self.assertEqual(getattr(material, name), (0.1, 0.2, 0.3, 1.0))
            setattr(material, name, (0.4, 0.5, 0.6, 0.7))
            self.assertEqual(getattr(material, name), (0.4, 0.5, 0.6, 0.7))

    def test_shininess_round_trips(self):
        material = FreeCAD.Material()
        material.Shininess = 0.75
        self.assertAlmostEqual(material.Shininess, 0.75)

    def test_transparency_is_independent_of_the_alpha_channel(self):
        # Upstream keeps `Transparency` and `DiffuseColor`'s alpha independent.
        material = FreeCAD.Material()
        material.Transparency = 0.25
        self.assertAlmostEqual(material.Transparency, 0.25)
        self.assertEqual(material.DiffuseColor[3], 1.0)
        material.DiffuseColor = (1.0, 0.0, 0.0, 0.5)
        self.assertAlmostEqual(material.Transparency, 0.25)
        with self.assertRaises(ValueError):
            material.Transparency = 1.5

    def test_equality_compares_every_field(self):
        a = FreeCAD.Material()
        b = FreeCAD.Material()
        self.assertEqual(a, b)
        b.Shininess = 0.1
        self.assertNotEqual(a, b)
        b = FreeCAD.Material()
        b.AmbientColor = (0.0, 0.0, 0.0, 1.0)
        self.assertNotEqual(a, b)

    def test_simplecads_clone_loop(self):
        # The exact loop SimpleCAD's appearance.clone performs over FIELDS.
        fields = (
            "DiffuseColor",
            "AmbientColor",
            "SpecularColor",
            "EmissiveColor",
            "Shininess",
            "Transparency",
        )
        source = FreeCAD.Material()
        source.DiffuseColor = (0.2, 0.4, 0.6)
        source.Shininess = 0.75
        source.Transparency = 0.25
        clone = FreeCAD.Material()
        for field in fields:
            setattr(clone, field, getattr(source, field))
        for field in fields:
            self.assertEqual(getattr(clone, field), getattr(source, field), field)


class MaterialViewWriteBackTests(unittest.TestCase):
    def setUp(self):
        self.doc = FreeCAD.newDocument("MaterialWriteBack")
        self.obj = self.doc.addObject("App::FeaturePython", "Body")

    def tearDown(self):
        FreeCAD.closeDocument(self.doc.Name)

    def test_diffuse_and_transparency_write_back_through_shape_appearance(self):
        material = self.obj.ViewObject.ShapeAppearance[0]
        material.DiffuseColor = (0.1, 0.2, 0.3, 1.0)
        self.assertEqual(
            self.obj.ViewObject.ShapeAppearance[0].DiffuseColor, (0.1, 0.2, 0.3, 1.0)
        )
        material.Transparency = 0.5
        self.assertAlmostEqual(
            self.obj.ViewObject.ShapeAppearance[0].Transparency, 0.5
        )
        # The first appearance slot is also the shape colour.
        self.assertEqual(self.obj.ViewObject.ShapeColor, (0.1, 0.2, 0.3, 0.5))

    def test_diffuse_and_transparency_write_back_through_shape_material(self):
        material = self.obj.ViewObject.ShapeMaterial
        material.DiffuseColor = (1.0, 0.0, 0.0, 1.0)
        self.assertEqual(self.obj.ViewObject.ShapeMaterial.DiffuseColor, (1.0, 0.0, 0.0, 1.0))
        material.Transparency = 0.4
        self.assertAlmostEqual(self.obj.ViewObject.ShapeMaterial.Transparency, 0.4)

    def test_owned_handle_accepts_all_material_fields(self):
        # SimpleCAD writes every field of an owned material before assigning the
        # list back; none of these may raise.
        material = self.obj.ViewObject.ShapeAppearance[0]
        material.AmbientColor = (0.1, 0.1, 0.1)
        material.SpecularColor = (0.2, 0.2, 0.2)
        material.EmissiveColor = (0.0, 0.0, 0.0)
        material.Shininess = 0.75
        self.assertAlmostEqual(material.Shininess, 0.75)

    def test_material_list_assignment_keeps_diffuse_and_transparency(self):
        first = FreeCAD.Material()
        first.DiffuseColor = (0.1, 0.2, 0.3)
        first.Transparency = 0.25
        second = FreeCAD.Material()
        second.DiffuseColor = (0.4, 0.5, 0.6)
        self.obj.ViewObject.ShapeAppearance = [first, second]
        values = self.obj.ViewObject.ShapeAppearance
        self.assertEqual(len(values), 2)
        self.assertEqual(values[0].DiffuseColor[:3], (0.1, 0.2, 0.3))
        self.assertAlmostEqual(values[0].Transparency, 0.25)
        self.assertEqual(values[1].DiffuseColor[:3], (0.4, 0.5, 0.6))
        self.assertAlmostEqual(values[1].Transparency, 0.0)


class SupportPropertyTests(unittest.TestCase):
    """SimpleCAD's `SC_Support` is an ordinary dynamic `App::PropertyLink`."""

    def setUp(self):
        self.doc = FreeCAD.newDocument("SupportLink")
        self.body = self.doc.addObject("App::FeaturePython", "Body")
        self.profile = self.doc.addObject("App::FeaturePython", "Profile")

    def tearDown(self):
        FreeCAD.closeDocument(self.doc.Name)

    def test_dynamic_link_round_trip(self):
        self.profile.addProperty(
            "App::PropertyLink", "SC_Support", "SimpleCAD", "Solid used by Push/Pull"
        )
        self.profile.SC_Support = self.body
        self.assertIn("SC_Support", self.profile.PropertiesList)
        self.assertIs(self.profile.SC_Support, self.body)
        self.profile.setEditorMode("SC_Support", 1)
        self.profile.removeProperty("SC_Support")
        self.assertNotIn("SC_Support", self.profile.PropertiesList)

    def test_missing_support_is_a_safe_getattr_default(self):
        # `visible_shapes` in SimpleCAD's operations.py reads it this way.
        self.assertIsNone(getattr(self.profile, "SC_Support", None))


if __name__ == "__main__":
    unittest.main()
