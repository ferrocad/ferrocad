"""Tests for the per-edition appearance preset layer and its reset surface.

The presets are a fallback **under** the user's persisted parameter values: a
declaration is validated against the capability schema, installing fills only
values the user has not chosen, and a provenance marker lets a later preset
change replace the values the previous preset wrote without touching a user
override. ``resetAppearance`` clears the capability store and reinstalls the
edition's value rather than the hard-coded default.

The edition modules (``ferrocad_toy`` / ``ferrocad_parametric``) are imported
lazily so this suite does not install their presets for the rest of the process;
each test restores the declarations it found.
"""

from __future__ import annotations

import colorsys
import importlib
import json
import unittest

import FreeCAD
import FreeCADGui
import FerroCAD
import FerroCADGui
import ferrocad_shell  # noqa: F401  (registers the reset commands)

from FerroCAD import _gizmo

GIZMO_PREFERENCES = "User parameter:BaseApp/Preferences/Mods/gizmo/ferrocad"
AXIS_PREFERENCES = "User parameter:BaseApp/Preferences/Mods/axis/ferrocad"
GRID_PREFERENCES = "User parameter:BaseApp/Preferences/Mods/grid/ferrocad"
LEGACY_GRID_PREFERENCES = "User parameter:BaseApp/Preferences/Mods/grid/ferrocad"
LEGACY_VIEW_PREFERENCES = "User parameter:BaseApp/Preferences/View"

RESET_COMMANDS = {
    "gizmo": "App_ResetGizmoAppearance",
    "axis": "App_ResetAxisAppearance",
    "grid": "App_ResetGridAppearance",
}

#: The toy slab's declared linear RGBA and the shell background's sRGB hex.
OBJECT_LINEAR = (0.26, 0.55, 0.85)
BACKGROUND_HEX = "#0f1218"


def _srgb(value: float) -> float:
    """The sRGB transfer function for one linear channel."""
    return 12.92 * value if value <= 0.0031308 else 1.055 * value ** (1 / 2.4) - 0.055


def _linear(value: float) -> float:
    """The inverse transfer function for one sRGB channel in ``0..1``."""
    return value / 12.92 if value <= 0.04045 else ((value + 0.055) / 1.055) ** 2.4


def _hex_rgb(text: str) -> tuple:
    text = text.lstrip("#")
    return tuple(int(text[index : index + 2], 16) / 255.0 for index in (0, 2, 4))


def _hex_linear(text: str) -> tuple:
    return tuple(_linear(channel) for channel in _hex_rgb(text))


def _luminance(text_or_linear) -> float:
    if isinstance(text_or_linear, str):
        rgb = _hex_linear(text_or_linear)
    else:
        rgb = text_or_linear
    return 0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]


def _hue(text: str) -> float:
    return colorsys.rgb_to_hsv(*_hex_rgb(text))[0] * 360.0


def _contrast(first: float, second: float) -> float:
    high, low = max(first, second), min(first, second)
    return (high + 0.05) / (low + 0.05)


def _hue_distance(first: float, second: float) -> float:
    difference = abs(first - second) % 360.0
    return min(difference, 360.0 - difference)


def _clear_store():
    for path in (GIZMO_PREFERENCES, AXIS_PREFERENCES, GRID_PREFERENCES):
        FreeCAD.ParamGet(path).Clear()
    FreeCAD.ParamGet(LEGACY_GRID_PREFERENCES).RemBool("GridAxisCross")
    FreeCAD.ParamGet(LEGACY_VIEW_PREFERENCES).RemBool("GridAxisCross")


class AppearancePresetTests(unittest.TestCase):
    def setUp(self):
        # The declarations other suites (test_namespace_separation) may have made
        # at import time are restored in tearDown; this suite owns the store.
        self.declared = FerroCADGui.appearancePresets()
        self.test_providers = []
        _clear_store()
        FerroCAD.Gizmo.reset()
        FerroCAD.Axis.reset()
        FerroCAD.Grid.reset()

    def tearDown(self):
        for feature in list(FerroCADGui.appearancePresets()):
            FerroCADGui.unregisterAppearancePreset(feature)
        for feature, values in self.declared.items():
            FerroCADGui.registerAppearancePreset(feature, values)
        FerroCAD.Gizmo.reset()
        FerroCAD.Axis.reset()
        FerroCAD.Grid.reset()
        _clear_store()
        for feature, backend in self.test_providers:
            FerroCADGui.unregisterProvider(feature, backend)

    # -- helpers -------------------------------------------------------------
    def register(self, feature, values):
        return FerroCADGui.registerAppearancePreset(feature, values)

    def install(self, feature=None):
        return FerroCADGui.installAppearancePresets(feature)

    def store(self, path):
        return FreeCAD.ParamGet(path)

    def use_edition(self, module_name):
        module = importlib.import_module(module_name)
        for feature, values in module.APPEARANCE_PRESETS.items():
            self.register(feature, values)
        return module

    # -- declarations --------------------------------------------------------
    def test_a_declaration_is_validated_and_writes_no_parameter(self):
        manifest = self.register(
            "gizmo",
            {"active_color": "#FFD9A3", "inactive_color": "#b08f6f"},
        )
        self.assertEqual(
            manifest,
            {
                "feature": "gizmo",
                "settings": {
                    "active_color": "#ffd9a3",
                    "inactive_color": "#b08f6f",
                },
            },
        )
        self.assertEqual(
            FerroCADGui.appearancePreset("gizmo")["active_color"], "#ffd9a3"
        )
        self.assertEqual(
            FerroCADGui.appearancePresets()["gizmo"]["inactive_color"], "#b08f6f"
        )
        self.assertNotIn("active_color", self.store(GIZMO_PREFERENCES).GetStrings())
        self.assertEqual(
            FerroCAD.Gizmo.active_color,
            _gizmo.DEFAULT_ACTIVE_COLOR,
            "declaring alone does not change the resolved policy",
        )

        # The returned manifest is a copy.
        manifest["settings"]["active_color"] = "#000000"
        self.assertEqual(
            FerroCADGui.appearancePreset("gizmo")["active_color"], "#ffd9a3"
        )

    def test_a_typo_or_a_bad_value_is_rejected_at_registration(self):
        before = FerroCADGui.appearancePreset("gizmo")
        with self.assertRaises(ValueError):
            self.register("gizmo", {"not_a_setting": 1})
        with self.assertRaises(ValueError):
            self.register("gizmo", {"active_color": "red"})
        with self.assertRaises(ValueError):
            self.register("gizmo", {"size": -4})
        with self.assertRaises(ValueError):
            self.register("", {"size": 10})
        with self.assertRaises(TypeError):
            self.register("gizmo", ["active_color"])
        self.assertEqual(FerroCADGui.appearancePreset("gizmo"), before)

    # -- precedence ----------------------------------------------------------
    def test_install_fills_only_the_unset_values(self):
        FerroCAD.Gizmo.size = 120.0  # the user's value, stored before install
        self.register(
            "gizmo",
            {"active_color": "#ffd9a3", "size": 96.0},
        )

        result = self.install("gizmo")
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["writes"], {"gizmo": 1})
        self.assertEqual(FerroCAD.Gizmo.active_color, "#ffd9a3")
        self.assertEqual(
            FerroCAD.Gizmo.size, 120.0, "the user's size outranks the preset"
        )

    def test_a_preset_change_replaces_its_values_but_not_user_overrides(self):
        self.register(
            "gizmo",
            {"active_color": "#111111", "inactive_color": "#222222"},
        )
        self.install("gizmo")
        self.assertEqual(FerroCAD.Gizmo.active_color, "#111111")

        FerroCAD.Gizmo.inactive_color = "#333333"  # a user override
        self.register(
            "gizmo",
            {"active_color": "#444444", "inactive_color": "#555555"},
        )
        self.install("gizmo")
        self.assertEqual(
            FerroCAD.Gizmo.active_color, "#444444", "the preset owned this value"
        )
        self.assertEqual(
            FerroCAD.Gizmo.inactive_color, "#333333", "the user's value survives"
        )

        # Dropping a key removes the value the preset owns; the built-in applies.
        self.register("gizmo", {"inactive_color": "#555555"})
        self.install("gizmo")
        self.assertEqual(FerroCAD.Gizmo.active_color, _gizmo.DEFAULT_ACTIVE_COLOR)

    def test_install_is_idempotent_and_unregister_reverts_its_values(self):
        self.register("gizmo", {"active_color": "#111111"})
        self.assertEqual(self.install("gizmo")["writes"], {"gizmo": 1})
        self.assertEqual(
            self.install("gizmo")["writes"], {"gizmo": 0}, "a second install is a no-op"
        )

        FerroCAD.Gizmo.inactive_color = "#333333"
        self.assertTrue(FerroCADGui.unregisterAppearancePreset("gizmo"))
        self.assertIsNone(FerroCADGui.appearancePreset("gizmo"))
        self.assertEqual(
            FerroCAD.Gizmo.active_color,
            _gizmo.DEFAULT_ACTIVE_COLOR,
            "the preset's own value is removed",
        )
        self.assertEqual(
            FerroCAD.Gizmo.inactive_color, "#333333", "the user's value is kept"
        )
        self.assertFalse(FerroCADGui.unregisterAppearancePreset("gizmo"))

    # -- reset ---------------------------------------------------------------
    def test_reset_restores_the_preset_not_the_builtin(self):
        self.register(
            "gizmo",
            {"active_color": "#ffd9a3", "inactive_color": "#b08f6f"},
        )
        self.install("gizmo")
        FerroCAD.Gizmo.active_color = "#000000"
        FerroCAD.Gizmo.inactive_color = "#000001"

        result = FerroCADGui.resetAppearance("gizmo")

        self.assertTrue(result["ok"], result)
        self.assertIn("edition preset", result["status"])
        self.assertEqual(result["view"]["gizmo"]["active_color"], "#ffd9a3")
        self.assertEqual(FerroCAD.Gizmo.active_color, "#ffd9a3")
        self.assertEqual(FerroCAD.Gizmo.inactive_color, "#b08f6f")
        self.assertIn(
            "__appearance_preset__", self.store(GIZMO_PREFERENCES).GetStrings()
        )

    def test_reset_without_an_argument_covers_the_three_capabilities(self):
        self.register("gizmo", {"active_color": "#ffd9a3"})
        self.register("axis", {"color_z": "#fbbf24"})
        self.register("grid", {"color": "#b8a894"})
        self.install()
        FerroCAD.Gizmo.active_color = "#000000"
        FerroCAD.Axis.color_z = "#000000"
        FerroCAD.Grid.color = "#000000"

        result = FerroCADGui.resetAppearance()

        self.assertTrue(result["ok"], result)
        self.assertEqual(result["features"], ["gizmo", "axis", "grid"])
        self.assertEqual(result["view"]["gizmo"]["active_color"], "#ffd9a3")
        self.assertEqual(result["view"]["axis"]["color_z"], "#fbbf24")
        self.assertEqual(result["view"]["grid_color"], "#b8a894")
        self.assertEqual(FerroCAD.Axis.color_z, "#fbbf24")
        self.assertEqual(FerroCAD.Grid.color, "#b8a894")

    def test_reset_clears_the_legacy_axis_flag_that_would_veto_it(self):
        # The old flag is migrated when ``enabled`` is unset. A reset must drop
        # it, or the pre-split user value would beat the edition's default.
        self.store(LEGACY_GRID_PREFERENCES).SetBool("GridAxisCross", False)
        self.store(LEGACY_VIEW_PREFERENCES).SetBool("GridAxisCross", False)
        self.register("axis", {"color_z": "#fbbf24"})

        result = FerroCADGui.resetAppearance("axis")

        self.assertTrue(result["ok"], result)
        self.assertTrue(FerroCAD.Axis.enabled, "the legacy False is gone")
        self.assertEqual(FerroCAD.Axis.color_z, "#fbbf24")
        self.assertNotIn(
            "GridAxisCross", self.store(LEGACY_VIEW_PREFERENCES).GetBools()
        )

    def test_an_unknown_feature_reports_instead_of_raising(self):
        result = FerroCADGui.resetAppearance("no-such-feature")
        self.assertFalse(result["ok"])
        self.assertTrue(result["errors"])

    # -- the edition presets and the contrast arithmetic ---------------------
    def test_the_toy_preset_contrasts_with_the_blue_object(self):
        module = self.use_edition("ferrocad_toy")
        preset = module.APPEARANCE_PRESETS["gizmo"]
        self.assertEqual(preset["active_color"], "#fbbf24")
        self.assertEqual(preset["inactive_color"], "#e2e8f0")

        result = self.install()
        self.assertTrue(result["ok"], result)
        self.assertEqual(FerroCAD.Gizmo.active_color, "#fbbf24")
        self.assertEqual(FerroCAD.Gizmo.inactive_color, "#e2e8f0")
        self.assertEqual(FerroCAD.Axis.color_z, "#fbbf24")
        self.assertEqual(FerroCAD.Grid.color, "#b8a894")

        object_y = _luminance(OBJECT_LINEAR)
        active_y = _luminance("#fbbf24")
        inactive_y = _luminance("#e2e8f0")
        default_y = _luminance(_gizmo.DEFAULT_ACTIVE_COLOR)
        background_y = _luminance(BACKGROUND_HEX)

        # The point of the preset: a warm active hue and a light idle ink, so the
        # stroke separates from the slab by hue and by lightness.
        object_hue = _hue(
            "".join("%02x" % round(_srgb(value) * 255) for value in OBJECT_LINEAR)
        )
        self.assertLess(
            _hue_distance(_hue(_gizmo.DEFAULT_ACTIVE_COLOR), object_hue),
            30,
            "the shared token sits in the object's hue family",
        )
        self.assertGreater(
            _hue_distance(_hue(preset["active_color"]), object_hue),
            120,
            "the toy active handle is on the opposite side of the hue wheel",
        )
        self.assertGreater(active_y, default_y)
        self.assertGreater(_contrast(active_y, background_y), 10.0)
        self.assertGreater(_contrast(inactive_y, background_y), 12.0)
        self.assertGreater(
            inactive_y, active_y, "the idle handle is the lightest ink"
        )
        self.assertGreater(
            _hue_distance(
                _hue(preset["active_color"]), _hue(preset["inactive_color"])
            ),
            120,
            "active and idle also differ by hue",
        )
        self.assertLess(_hue(preset["active_color"]), 60.0, "warm, not magenta")
        self.assertLess(_hue("#b8a894"), 60.0)

        # The shared halo is what carries SC 1.4.11 over the slab: the
        # outline/object and outline/stroke boundaries all clear 3:1.
        outline_y = _luminance(_gizmo.DEFAULT_OUTLINE_COLOR)
        self.assertGreaterEqual(_contrast(outline_y, object_y), 3.0)
        self.assertGreaterEqual(_contrast(outline_y, active_y), 3.0)
        self.assertGreaterEqual(_contrast(outline_y, inactive_y), 3.0)

    def test_the_parametric_preset_pins_the_neutral_look(self):
        try:
            module = importlib.import_module("ferrocad_parametric")
        except ImportError:
            self.skipTest("Part is not in this extension build")
        preset = module.APPEARANCE_PRESETS["gizmo"]
        self.assertEqual(preset["active_color"], _gizmo.DEFAULT_ACTIVE_COLOR)
        self.assertEqual(preset["inactive_color"], _gizmo.DEFAULT_INACTIVE_COLOR)
        self.assertEqual(
            module.APPEARANCE_PRESETS["axis"]["color_z"], "#38bdf8"
        )
        self.assertEqual(module.APPEARANCE_PRESETS["grid"]["color"], "#94a3b8")

        for feature, values in module.APPEARANCE_PRESETS.items():
            self.register(feature, values)
        self.install()
        self.assertEqual(FerroCAD.Gizmo.active_color, _gizmo.DEFAULT_ACTIVE_COLOR)
        self.assertEqual(FerroCAD.Axis.color_z, "#38bdf8")
        self.assertEqual(FerroCAD.Grid.color, "#94a3b8")

    # -- the command and settings-surface hook -------------------------------
    def test_the_reset_commands_are_registered_and_return_a_view(self):
        self.register("gizmo", {"active_color": "#ffd9a3"})
        self.install("gizmo")
        FerroCAD.Gizmo.active_color = "#000000"

        for command in RESET_COMMANDS.values():
            self.assertIn(command, FreeCADGui.listCommands())

        result = FreeCADGui._run("App_ResetGizmoAppearance")
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["view"]["gizmo"]["active_color"], "#ffd9a3")

    def test_the_settings_rows_carry_the_reset_action(self):
        self.register("gizmo", {"active_color": "#ffd9a3"})
        feature = "gizmo"
        backend = "test-appearance"
        FerroCADGui.registerProvider(
            feature,
            backend,
            "Test appearance",
            settings=_gizmo.GIZMO_SETTINGS_SCHEMA,
        )
        self.test_providers.append((feature, backend))

        rows = json.loads(ferrocad_shell.properties("Settings/Gizmo"))
        reset = next(row for row in rows if row["name"] == "reset")
        self.assertEqual(reset["kind"], "command")
        self.assertEqual(reset["command"], RESET_COMMANDS["gizmo"])
        self.assertFalse(reset["editable"])
        self.assertEqual(reset["status"], "action")

        # A generic text edit of the action row routes to the same surface.
        self.install("gizmo")
        FerroCAD.Gizmo.active_color = "#000000"
        result = json.loads(
            ferrocad_shell.set_property("Settings/Gizmo", "reset", "")
        )
        self.assertTrue(result["ok"], result)
        self.assertEqual(result["view"]["gizmo"]["active_color"], "#ffd9a3")

    def test_the_shell_boot_bridge_returns_the_resolved_view(self):
        self.register("gizmo", {"active_color": "#ffd9a3"})
        payload = json.loads(ferrocad_shell.appearance_view())
        self.assertTrue(payload["ok"], payload)
        self.assertEqual(payload["error"], "")
        self.assertEqual(payload["errors"], [])
        self.assertEqual(payload["view"]["gizmo"]["active_color"], "#ffd9a3")


if __name__ == "__main__":
    unittest.main()
