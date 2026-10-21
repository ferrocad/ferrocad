"""Tests for the semantic grid API, ``FerroCAD.Grid`` (G2, Track C).

The API is pure Python over ``FreeCAD.ParamGet`` and is exercised headless
(``FreeCAD.GuiUp == 0``): every setter validates and stores, ``apply()`` returns
the shell's ``{"view": …}`` request and never raises, and ``snapshot()`` reads
the store back. See ``docs/workbenches/grid-overlay-plan.md`` §2.
"""

import math
import unittest

import FreeCAD
import FerroCAD

PREFERENCES = "User parameter:BaseApp/Preferences/View"

VIEW_KEYS = {
    "grid",
    "axis_cross",
    "grid_minor",
    "grid_major",
    "grid_radius",
    "grid_color",
    "grid_alpha",
}


class GridApiTests(unittest.TestCase):
    def setUp(self):
        self.grid = FerroCAD.Grid
        self.grid.reset()
        self.prefs = FreeCAD.ParamGet(PREFERENCES)

    def tearDown(self):
        self.grid.reset()

    def test_the_singleton_is_lazy_and_stable(self):
        self.assertIs(FerroCAD.Grid, self.grid)
        self.assertIs(FerroCAD.Grid, getattr(FerroCAD, "Grid"))
        self.assertIn("Grid", FerroCAD.__all__)

    def test_the_defaults_are_the_plan_s_defaults(self):
        self.assertFalse(self.grid.enabled)
        self.assertEqual(self.grid.color, "#94a3b8")
        self.assertEqual(self.grid.opacity, 1.0)
        self.assertEqual(self.grid.spacing, 10.0)
        self.assertEqual(self.grid.minor, 10.0)
        self.assertEqual(self.grid.major, 100.0)
        self.assertEqual(self.grid.radius, 0.0)
        self.assertTrue(self.grid.axis_cross)
        self.assertFalse(self.grid.shadows)
        self.assertAlmostEqual(self.grid.shadow_opacity, 0.30)

    def test_the_store_uses_the_documented_keys(self):
        self.grid.enabled = True
        self.grid.color = "#94A3B8"
        self.grid.opacity = 0.65
        self.grid.spacing = 2.5
        self.grid.radius = 750.0
        self.grid.axis_cross = False
        self.grid.shadows = True
        self.grid.shadow_opacity = 0.4

        self.assertTrue(self.prefs.GetBool("GridEnabled", False))
        self.assertEqual(
            self.prefs.GetString("GridColor", ""), "#94a3b8", "canonical spelling"
        )
        self.assertAlmostEqual(self.prefs.GetFloat("GridOpacity", 0.0), 0.65)
        self.assertAlmostEqual(self.prefs.GetFloat("GridMinorStep", 0.0), 2.5)
        self.assertAlmostEqual(
            self.prefs.GetFloat("GridMajorStep", 0.0),
            25.0,
            msg="the major step is the next decade",
        )
        self.assertAlmostEqual(self.prefs.GetFloat("GridRadius", 0.0), 750.0)
        self.assertFalse(self.prefs.GetBool("GridAxisCross", True))
        self.assertTrue(self.prefs.GetBool("GridShadow", False))
        self.assertAlmostEqual(self.prefs.GetFloat("GridShadowOpacity", 0.0), 0.4)

    def test_each_property_round_trips(self):
        self.grid.enabled = True
        self.assertTrue(self.grid.enabled)

        self.grid.color = "#F00"
        self.assertEqual(self.grid.color, "#ff0000", "a short colour is expanded")
        self.grid.color = "#123456"
        self.assertEqual(self.grid.color, "#123456")

        self.grid.opacity = 0.0
        self.assertEqual(self.grid.opacity, 0.0)
        self.grid.opacity = 1.0
        self.assertEqual(self.grid.opacity, 1.0)

        self.grid.minor = 2.0
        self.grid.major = 20.0
        self.assertEqual((self.grid.minor, self.grid.major), (2.0, 20.0))

        # `spacing` is the convenience that writes the minor step and the next decade.
        self.grid.spacing = 0.5
        self.assertEqual(self.grid.spacing, 0.5)
        self.assertEqual((self.grid.minor, self.grid.major), (0.5, 5.0))

        self.grid.radius = 0.0
        self.assertEqual(self.grid.radius, 0.0)
        self.grid.radius = 420.0
        self.assertEqual(self.grid.radius, 420.0)

        self.grid.axis_cross = False
        self.assertFalse(self.grid.axis_cross)
        self.grid.shadows = True
        self.assertTrue(self.grid.shadows)
        self.grid.shadow_opacity = 0.0
        self.assertEqual(self.grid.shadow_opacity, 0.0)

    def test_a_bad_colour_raises_and_leaves_the_store_untouched(self):
        for bad in ("", "red", "#12", "#1234", "#1234567", "#gggggg", "#12345g", 42, None):
            with self.assertRaises(ValueError, msg=repr(bad)):
                self.grid.color = bad
            self.assertEqual(self.grid.color, "#94a3b8")

    def test_bad_numbers_raise_and_leave_the_store_untouched(self):
        for value in (0, -1, -0.5, float("nan"), float("inf")):
            for name in ("minor", "major", "spacing"):
                with self.assertRaises(ValueError, msg="%s=%r" % (name, value)):
                    setattr(self.grid, name, value)
            self.assertEqual((self.grid.minor, self.grid.major), (10.0, 100.0))

        for value in (-0.1, 1.1, float("nan"), float("inf")):
            for name in ("opacity", "shadow_opacity"):
                with self.assertRaises(ValueError, msg="%s=%r" % (name, value)):
                    setattr(self.grid, name, value)

        for value in (-0.1, float("nan"), float("inf")):
            with self.assertRaises(ValueError, msg="radius=%r" % (value,)):
                self.grid.radius = value

        self.assertEqual(self.grid.opacity, 1.0)
        self.assertAlmostEqual(self.grid.shadow_opacity, 0.30)
        self.assertEqual(self.grid.radius, 0.0)

    def test_apply_builds_only_the_keys_the_shell_understands(self):
        self.grid.enabled = True
        self.grid.color = "#f87171"
        self.grid.opacity = 0.5
        self.grid.minor = 2.0
        self.grid.major = 20.0
        self.grid.radius = 750.0
        self.grid.axis_cross = False
        self.grid.shadows = True

        self.assertEqual(
            self.grid.apply(),
            {
                "view": {
                    "grid": True,
                    "axis_cross": False,
                    "grid_minor": 2.0,
                    "grid_major": 20.0,
                    "grid_radius": 750.0,
                    "grid_color": "#f87171",
                    "grid_alpha": 0.5,
                }
            },
        )
        self.assertEqual(set(self.grid.apply()["view"]), VIEW_KEYS)
        self.assertNotIn("shadow", " ".join(self.grid.apply()["view"]))

    def test_apply_defaults_are_the_token_state(self):
        self.assertEqual(
            self.grid.apply(),
            {
                "view": {
                    "grid": False,
                    "axis_cross": True,
                    "grid_minor": 10.0,
                    "grid_major": 100.0,
                    "grid_radius": 0.0,
                    "grid_color": "#94a3b8",
                    "grid_alpha": 1.0,
                }
            },
        )

    def test_apply_rejects_a_major_below_the_minor(self):
        # Each step is valid on its own; the pair is what apply() refuses.
        self.grid.minor = 50.0
        self.grid.major = 5.0
        with self.assertRaises(ValueError) as caught:
            self.grid.apply()
        self.assertIn("major", str(caught.exception))

    def test_apply_does_not_raise_headless(self):
        self.assertEqual(FreeCAD.GuiUp, 0, "this suite runs headless")
        self.assertIsInstance(self.grid.apply()["view"], dict)

    def test_snapshot_reads_the_store_headless(self):
        self.assertEqual(FreeCAD.GuiUp, 0)
        snapshot = self.grid.snapshot()
        self.assertTrue(VIEW_KEYS <= set(snapshot))
        self.assertEqual(
            set(snapshot) - VIEW_KEYS,
            {"shadows", "shadow_opacity"},
            "the snapshot reports the deferred knobs too",
        )
        self.assertFalse(snapshot["grid"])
        self.assertTrue(snapshot["axis_cross"])
        self.assertEqual(snapshot["grid_minor"], 10.0)
        self.assertEqual(snapshot["grid_major"], 100.0)
        self.assertIsNone(snapshot["grid_radius"], "0 is the camera-derived extent")
        self.assertEqual(snapshot["grid_color"], "#94a3b8")
        self.assertEqual(snapshot["grid_alpha"], 1.0)

        # A policy write is visible before it is applied, and the snapshot keeps
        # the same shape as the request.
        self.grid.minor = 5.0
        self.grid.major = 50.0
        self.grid.radius = 250.0
        self.grid.color = "#00ff00"
        self.grid.opacity = 0.25
        snapshot = self.grid.snapshot()
        self.assertEqual(snapshot["grid_minor"], 5.0)
        self.assertEqual(snapshot["grid_major"], 50.0)
        self.assertEqual(snapshot["grid_radius"], 250.0)
        self.assertEqual(snapshot["grid_color"], "#00ff00")
        self.assertEqual(snapshot["grid_alpha"], 0.25)
        self.assertTrue(set(self.grid.apply()["view"]) <= set(snapshot))

    def test_reset_clears_the_group(self):
        self.grid.enabled = True
        self.grid.color = "#000000"
        self.grid.minor = 3.0
        self.grid.major = 30.0
        self.grid.shadow_opacity = 0.1

        self.grid.reset()

        self.assertEqual(self.prefs.GetString("GridColor", ""), "")
        self.assertEqual(self.prefs.GetFloat("GridMinorStep", -1.0), -1.0)
        self.assertFalse(self.grid.enabled)
        self.assertEqual(self.grid.color, "#94a3b8")
        self.assertEqual((self.grid.minor, self.grid.major), (10.0, 100.0))
        self.assertAlmostEqual(self.grid.shadow_opacity, 0.30)

    def test_a_non_finite_step_is_not_clamped(self):
        self.grid.minor = 4.0
        with self.assertRaises(ValueError):
            self.grid.minor = math.nan
        self.assertEqual(self.grid.minor, 4.0)


class GridSettingsCommandTests(unittest.TestCase):
    """``View_GridSettings`` is a thin client of ``FerroCAD.Grid``."""

    def setUp(self):
        self.grid = FerroCAD.Grid
        self.grid.reset()
        import FreeCADGui

        self.gui = FreeCADGui
        import ferrocad_shell  # noqa: F401  (registers View_GridSettings)

    def tearDown(self):
        self.gui._cancel_all()
        self.grid.reset()

    def test_the_dialog_accept_returns_the_view_request(self):
        run = self.gui._run("View_GridSettings")
        self.assertTrue(run["ok"], run)
        fields = {field["name"] for field in run["dialog"]["fields"]}
        self.assertEqual(
            fields,
            {"enabled", "color", "opacity", "spacing", "shadows", "shadow_opacity"},
        )

        accepted = self.gui._accept(
            run["token"],
            {
                "enabled": True,
                "color": "#123456",
                "opacity": 0.5,
                "spacing": 2.0,
                "shadows": True,
                "shadow_opacity": 0.25,
            },
        )
        self.assertTrue(accepted["ok"], accepted)
        self.assertEqual(accepted["view"]["grid"], True)
        self.assertEqual(accepted["view"]["grid_color"], "#123456")
        self.assertEqual(accepted["view"]["grid_alpha"], 0.5)
        self.assertEqual(accepted["view"]["grid_minor"], 2.0)
        self.assertEqual(accepted["view"]["grid_major"], 20.0)
        self.assertTrue(self.grid.shadows, "the shadow preference persists")
        self.assertEqual(self.grid.shadow_opacity, 0.25)

    def test_a_bad_colour_keeps_the_dialog_open_with_the_error(self):
        run = self.gui._run("View_GridSettings")
        accepted = self.gui._accept(run["token"], {"color": "not a colour"})
        self.assertFalse(accepted["ok"])
        self.assertIn("colour", accepted["error"])


if __name__ == "__main__":
    unittest.main()
