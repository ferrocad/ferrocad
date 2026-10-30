"""Tests for the semantic grid API, ``FerroCAD.Grid`` (G2, Track C).

The API is pure Python over ``FreeCAD.ParamGet`` and is exercised headless
(``FreeCAD.GuiUp == 0``): every setter validates and stores, ``apply()`` returns
the shell's ``{"view": …}`` request and never raises, and ``snapshot()`` reads
the store back. See ``docs/developer/workbenches/grid-overlay-plan.md`` §2.
"""

import math
import unittest

import FreeCAD
import FerroCAD

PREFERENCES = "User parameter:BaseApp/Preferences/Mods/grid/ferrocad"
LEGACY_PREFERENCES = "User parameter:BaseApp/Preferences/View"

VIEW_KEYS = {
    "grid",
    "grid_minor",
    "grid_major",
    "grid_adaptive",
    "grid_minor_style",
    "grid_major_width",
    "grid_minor_width",
    "grid_radius",
    "grid_color",
    "grid_alpha",
}


class GridApiTests(unittest.TestCase):
    def setUp(self):
        self.grid = FerroCAD.Grid
        self.grid.reset()
        self.prefs = FreeCAD.ParamGet(PREFERENCES)
        self.legacy = FreeCAD.ParamGet(LEGACY_PREFERENCES)

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
        self.assertTrue(
            self.grid.adaptive, "adaptive spacing is the shipped look"
        )
        self.assertEqual(self.grid.minor_style, "dots")
        self.assertEqual(self.grid.major_width, 1.0)
        self.assertEqual(self.grid.minor_width, 1.0)
        self.assertEqual(self.grid.spacing, 10.0)
        self.assertEqual(self.grid.minor, 10.0)
        self.assertEqual(self.grid.major, 100.0)
        self.assertEqual(self.grid.radius, 0.0)
        self.assertFalse(self.grid.shadows)
        self.assertAlmostEqual(self.grid.shadow_opacity, 0.30)

    def test_the_store_uses_the_documented_keys(self):
        self.grid.enabled = True
        self.grid.color = "#94A3B8"
        self.grid.opacity = 0.65
        self.grid.spacing = 2.5
        self.grid.radius = 750.0
        self.grid.shadows = True
        self.grid.shadow_opacity = 0.4

        self.assertTrue(self.prefs.GetBool("enabled", False))
        self.assertEqual(
            self.prefs.GetString("color", ""), "#94a3b8", "canonical spelling"
        )
        self.assertAlmostEqual(self.prefs.GetFloat("opacity", 0.0), 0.65)
        self.assertAlmostEqual(self.prefs.GetFloat("minor", 0.0), 2.5)
        self.assertAlmostEqual(
            self.prefs.GetFloat("major", 0.0),
            25.0,
            msg="the major step is the next decade",
        )
        self.assertAlmostEqual(self.prefs.GetFloat("radius", 0.0), 750.0)
        self.assertTrue(self.prefs.GetBool("shadows", False))
        self.assertAlmostEqual(self.prefs.GetFloat("shadow_opacity", 0.0), 0.4)

    def test_each_property_round_trips(self):
        self.grid.enabled = True
        self.assertTrue(self.grid.enabled)

        self.grid.adaptive = False
        self.assertFalse(self.grid.adaptive)
        self.assertFalse(
            self.prefs.GetBool("adaptive", True), "one typed key per knob"
        )
        self.assertFalse(self.grid.apply()["view"]["grid_adaptive"])
        self.grid.adaptive = True
        self.assertTrue(self.grid.apply()["view"]["grid_adaptive"])

        self.grid.minor_style = "LINES"
        self.assertEqual(self.grid.minor_style, "lines", "the form is normalised")
        self.assertEqual(self.grid.apply()["view"]["grid_minor_style"], "lines")
        self.grid.minor_style = "dots"

        self.grid.major_width = 3
        self.grid.minor_width = 2.5
        self.assertEqual(self.grid.major_width, 3.0)
        self.assertEqual(self.grid.minor_width, 2.5)
        view = self.grid.apply()["view"]
        self.assertEqual((view["grid_major_width"], view["grid_minor_width"]), (3.0, 2.5))

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

    def test_a_bad_stroke_raises_and_leaves_the_store_untouched(self):
        for bad in ("dashes", "", "dot", 3, None):
            with self.assertRaises(ValueError, msg=repr(bad)):
                self.grid.minor_style = bad
        self.assertEqual(self.grid.minor_style, "dots")

        for value in (0, 0.5, 9, -2, float("nan"), float("inf")):
            for name in ("major_width", "minor_width"):
                with self.assertRaises(ValueError, msg="%s=%r" % (name, value)):
                    setattr(self.grid, name, value)
        self.assertEqual((self.grid.major_width, self.grid.minor_width), (1.0, 1.0))

    def test_apply_builds_only_the_keys_the_shell_understands(self):
        self.grid.enabled = True
        self.grid.color = "#f87171"
        self.grid.opacity = 0.5
        self.grid.minor = 2.0
        self.grid.major = 20.0
        self.grid.radius = 750.0
        self.grid.shadows = True

        self.assertEqual(
            self.grid.apply(),
            {
                "view": {
                    "grid": True,
                    "grid_minor": 2.0,
                    "grid_major": 20.0,
                    "grid_adaptive": True,
                    "grid_minor_style": "dots",
                    "grid_major_width": 1.0,
                    "grid_minor_width": 1.0,
                    "grid_radius": 750.0,
                    "grid_color": "#f87171",
                    "grid_alpha": 0.5,
                }
            },
        )
        self.assertEqual(set(self.grid.apply()["view"]), VIEW_KEYS)
        self.assertNotIn("shadow", " ".join(self.grid.apply()["view"]))

    def test_the_schema_advertises_adaptive_spacing(self):
        # The Settings tree renders this row as a checkbox; the store key is the
        # attribute's own name.
        entry = next(
            item for item in self.grid.schema() if item["name"] == "adaptive"
        )
        self.assertEqual(entry["kind"], "bool")
        self.assertEqual(entry["label"], "Adaptive spacing")
        self.assertTrue(entry["default"])

    def test_apply_defaults_are_the_token_state(self):
        self.assertEqual(
            self.grid.apply(),
            {
                "view": {
                    "grid": False,
                    "grid_minor": 10.0,
                    "grid_major": 100.0,
                    "grid_adaptive": True,
                    "grid_minor_style": "dots",
                    "grid_major_width": 1.0,
                    "grid_minor_width": 1.0,
                    "grid_radius": 0.0,
                    "grid_color": "#94a3b8",
                    "grid_alpha": 1.0,
                }
            },
        )

    def test_a_major_below_the_minor_is_refused(self):
        # Each step is valid on its own; the pair is what matters.
        self.grid.minor = 50.0
        with self.assertRaises(ValueError) as caught:
            self.grid.major = 5.0
        self.assertIn("major", str(caught.exception))
        self.assertGreaterEqual(
            self.grid.major, self.grid.minor, "a rejected write leaves the pair usable"
        )

    def test_an_inverted_store_is_healed_rather_than_refused(self):
        # A legacy key or an external edit can leave the pair inverted. `apply()` must
        # still answer: raising would send *no* view request, and the viewport would
        # keep drawing whatever it had (a disabled grid staying on screen).
        self.prefs.SetFloat("minor", 590.036)
        self.prefs.SetFloat("major", 169.926)
        view = self.grid.apply()["view"]
        self.assertEqual(view["grid_minor"], 590.036)
        self.assertGreaterEqual(view["grid_major"], view["grid_minor"])

    def test_raising_the_minor_past_the_major_steps_the_major_up(self):
        self.grid.minor = 10.0
        self.grid.major = 100.0
        self.grid.minor = 250.0
        self.assertEqual(
            self.grid.major, 2500.0, "the next decade, not a pair apply() would refuse"
        )
        self.grid.apply()  # never raises

    def test_a_major_equal_to_the_minor_lowers_the_minor_a_decade(self):
        """The levels are a subdivision apart, so `minor == major` is not a state.

        A minor step equal to the major one has no rows of its own — every lattice row
        at that step *is* a major row — so the minor level vanishes and `minor_style`
        and `minor_width` silently do nothing. Typing the same number into the major
        row means "a grid at this pitch, subdivided a decade below".
        """
        self.grid.minor = 10.0
        self.grid.major = 10.0
        self.assertEqual((self.grid.minor, self.grid.major), (1.0, 10.0))
        view = self.grid.apply()["view"]
        self.assertLess(view["grid_minor"], view["grid_major"])

    def test_a_stored_degenerate_pair_is_healed_and_written_back(self):
        # The same state can arrive from a legacy key or an external edit.
        self.prefs.SetFloat("minor", 10.0)
        self.prefs.SetFloat("major", 10.0)
        view = self.grid.apply()["view"]
        self.assertEqual((view["grid_minor"], view["grid_major"]), (1.0, 10.0))
        self.assertEqual(
            (self.grid.minor, self.grid.major),
            (1.0, 10.0),
            "the repair is durable, so the pane and the drawing agree",
        )

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
        self.assertEqual(snapshot["grid_minor"], 10.0)
        self.assertEqual(snapshot["grid_major"], 100.0)
        self.assertTrue(snapshot["grid_adaptive"])
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

    def test_reset_clears_the_store_and_the_legacy_keys(self):
        self.grid.enabled = True
        self.grid.color = "#000000"
        self.grid.minor = 3.0
        self.grid.major = 30.0
        self.grid.shadow_opacity = 0.1
        self.legacy.SetBool("GridEnabled", True)
        self.legacy.SetString("GridColor", "#123456")

        self.grid.reset()

        self.assertEqual(self.prefs.GetString("color", ""), "")
        self.assertEqual(self.prefs.GetFloat("minor", -1.0), -1.0)
        self.assertEqual(self.prefs.GetBools(), [])
        self.assertEqual(self.prefs.GetStrings(), [])
        self.assertEqual(self.prefs.GetFloats(), [])
        self.assertEqual(
            (
                self.legacy.GetString("GridColor", ""),
                self.legacy.GetFloat("GridMinorStep", -1.0),
                self.legacy.GetBools(),
            ),
            ("", -1.0, []),
            "reset removes the grid's legacy keys, not the whole View group",
        )
        self.assertFalse(self.grid.enabled)
        self.assertEqual(self.grid.color, "#94a3b8")
        self.assertEqual((self.grid.minor, self.grid.major), (10.0, 100.0))
        self.assertAlmostEqual(self.grid.shadow_opacity, 0.30)

    def test_a_legacy_value_migrates_once(self):
        self.legacy.SetBool("GridEnabled", True)
        self.legacy.SetString("GridColor", "#AABBCC")
        self.legacy.SetFloat("GridMinorStep", 2.0)
        self.legacy.SetFloat("GridMajorStep", 20.0)

        self.assertTrue(self.grid.enabled)
        self.assertEqual(self.grid.color, "#aabbcc")
        self.assertEqual((self.grid.minor, self.grid.major), (2.0, 20.0))
        self.assertEqual(
            self.prefs.GetBools(),
            ["enabled"],
            "the first read copies the legacy value into the provider group",
        )

        # The provider group is authoritative once migrated: a later change to
        # the legacy key is not read again.
        self.legacy.SetFloat("GridMinorStep", 99.0)
        self.assertEqual(self.grid.minor, 2.0)

        # An unrelated View preference is never touched by the migration.
        self.legacy.SetBool("SomeOtherPreference", True)
        self.assertEqual(self.grid.radius, 0.0)
        self.assertTrue(self.legacy.GetBool("SomeOtherPreference", False))
        self.legacy.RemBool("SomeOtherPreference")

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
