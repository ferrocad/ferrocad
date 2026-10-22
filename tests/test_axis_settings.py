"""Tests for the semantic axis-cross API, ``FerroCAD.Axis``.

The axis cross was split out of the grid capability: ``FerroCAD.Axis`` owns its
own schema and parameter group, while ``FerroCAD.Grid.axis_cross`` remains as a
deprecated proxy. The suite is headless (``FreeCAD.GuiUp == 0``): every setter
validates and stores, ``apply()`` returns the shell's ``{"view": …}`` request and
never raises, and ``snapshot()`` reads the store back.
"""

import importlib.util
import json
import math
import pathlib
import unittest
from unittest import mock

import FreeCAD
import FreeCADGui
import FerroCAD
import FerroCADGui
import ferrocad_shell  # noqa: F401  (registers App_ToggleAxisCross)
from FerroCAD import _axis

PREFERENCES = "User parameter:BaseApp/Preferences/Mods/axis/ferrocad"
GRID_PREFERENCES = "User parameter:BaseApp/Preferences/Mods/grid/ferrocad"
LEGACY_PREFERENCES = "User parameter:BaseApp/Preferences/View"

MOD_PATH = (
    pathlib.Path(__file__).resolve().parents[1]
    / "mods"
    / "AxisOverlay"
    / "InitGui.py"
)
COMMAND = "AxisOverlay_Settings"

#: The canonical schema, in order, with each default.
ATTRIBUTES = {
    "enabled": True,
    "color_x": "#f87171",
    "color_y": "#4ade80",
    "color_z": "#38bdf8",
    "opacity": 0.65,
    "length": 80.0,
    "width": 1.5,
    "origin": True,
    "labels": True,
    "ghost_alpha": 0.10,
}

VIEW_KEYS = set(ATTRIBUTES)


def clear_preferences():
    FreeCAD.ParamGet(PREFERENCES).Clear()
    FreeCAD.ParamGet(GRID_PREFERENCES).RemBool("GridAxisCross")
    FreeCAD.ParamGet(LEGACY_PREFERENCES).RemBool("GridAxisCross")


class AxisApiTests(unittest.TestCase):
    def setUp(self):
        self.axis = FerroCAD.Axis
        self.axis.reset()
        clear_preferences()
        self.prefs = FreeCAD.ParamGet(PREFERENCES)

    def tearDown(self):
        self.axis.reset()
        clear_preferences()

    def test_the_singleton_is_lazy_and_stable(self):
        self.assertIs(FerroCAD.Axis, self.axis)
        self.assertIs(FerroCAD.Axis, getattr(FerroCAD, "Axis"))
        self.assertIn("Axis", FerroCAD.__all__)

    def test_the_schema_is_the_shared_contract(self):
        self.assertEqual(
            [entry["name"] for entry in _axis.SETTINGS_SCHEMA], list(ATTRIBUTES)
        )
        self.assertEqual(
            [(entry["name"], entry["default"]) for entry in _axis.SETTINGS_SCHEMA],
            list(ATTRIBUTES.items()),
        )
        kinds = {entry["name"]: entry["kind"] for entry in _axis.SETTINGS_SCHEMA}
        self.assertEqual(
            kinds,
            {
                "enabled": "bool",
                "color_x": "color",
                "color_y": "color",
                "color_z": "color",
                "opacity": "float",
                "length": "float",
                "width": "float",
                "origin": "bool",
                "labels": "bool",
                "ghost_alpha": "float",
            },
        )
        bounds = {
            entry["name"]: (entry.get("min"), entry.get("max"))
            for entry in _axis.SETTINGS_SCHEMA
            if entry["kind"] == "float"
        }
        self.assertEqual(
            bounds,
            {
                "opacity": (0.0, 1.0),
                "length": (8.0, 400.0),
                "width": (0.5, 6.0),
                "ghost_alpha": (0.0, 1.0),
            },
        )

        # ``schema()`` is a copy: a caller cannot mutate the canonical list.
        schema = self.axis.schema()
        schema[0]["name"] = "mutated"
        self.assertEqual(_axis.SETTINGS_SCHEMA[0]["name"], "enabled")

    def test_the_defaults_are_the_contract_defaults(self):
        self.assertTrue(self.axis.enabled)
        self.assertEqual(self.axis.color_x, "#f87171")
        self.assertEqual(self.axis.color_y, "#4ade80")
        self.assertEqual(self.axis.color_z, "#38bdf8")
        self.assertEqual(self.axis.opacity, 0.65)
        self.assertEqual(self.axis.length, 80.0)
        self.assertEqual(self.axis.width, 1.5)
        self.assertTrue(self.axis.origin)
        self.assertTrue(self.axis.labels)
        self.assertAlmostEqual(self.axis.ghost_alpha, 0.10)

    def test_the_store_uses_the_schema_names(self):
        self.axis.enabled = False
        self.axis.color_x = "#FF0000"
        self.axis.color_y = "#0f0"
        self.axis.color_z = "#0000FF"
        self.axis.opacity = 0.25
        self.axis.length = 120.0
        self.axis.width = 3.5
        self.axis.origin = False
        self.axis.labels = False
        self.axis.ghost_alpha = 0.4

        self.assertFalse(self.prefs.GetBool("enabled", True))
        self.assertEqual(self.prefs.GetString("color_x", ""), "#ff0000")
        self.assertEqual(self.prefs.GetString("color_y", ""), "#00ff00")
        self.assertEqual(self.prefs.GetString("color_z", ""), "#0000ff")
        self.assertAlmostEqual(self.prefs.GetFloat("opacity", -1.0), 0.25)
        self.assertAlmostEqual(self.prefs.GetFloat("length", -1.0), 120.0)
        self.assertAlmostEqual(self.prefs.GetFloat("width", -1.0), 3.5)
        self.assertFalse(self.prefs.GetBool("origin", True))
        self.assertFalse(self.prefs.GetBool("labels", True))
        self.assertAlmostEqual(self.prefs.GetFloat("ghost_alpha", -1.0), 0.4)

    def test_each_property_round_trips(self):
        self.axis.enabled = False
        self.assertFalse(self.axis.enabled)

        self.axis.color_x = "#F00"
        self.assertEqual(self.axis.color_x, "#ff0000")
        self.axis.color_y = "#0F0"
        self.assertEqual(self.axis.color_y, "#00ff00")
        self.axis.color_z = "#00F"
        self.assertEqual(self.axis.color_z, "#0000ff")

        for name, value in (
            ("opacity", 0.0),
            ("opacity", 1.0),
            ("length", 8.0),
            ("length", 400.0),
            ("width", 0.5),
            ("width", 6.0),
            ("ghost_alpha", 0.0),
            ("ghost_alpha", 1.0),
        ):
            setattr(self.axis, name, value)
            self.assertEqual(getattr(self.axis, name), value)

        self.axis.origin = False
        self.assertFalse(self.axis.origin)
        self.axis.labels = False
        self.assertFalse(self.axis.labels)

    def test_a_bad_colour_raises_and_leaves_the_store_untouched(self):
        for bad in ("", "red", "#12", "#1234", "#1234567", "#gggggg", "#12345g", 42, None):
            for name in ("color_x", "color_y", "color_z"):
                with self.assertRaises(ValueError, msg="%s=%r" % (name, bad)):
                    setattr(self.axis, name, bad)
        self.assertEqual(
            (self.axis.color_x, self.axis.color_y, self.axis.color_z),
            ("#f87171", "#4ade80", "#38bdf8"),
        )

    def test_bad_numbers_raise_and_leave_the_store_untouched(self):
        cases = {
            "opacity": (-0.1, 1.1, math.nan, math.inf),
            "length": (7.9, 400.1, 0.0, -1.0, math.nan, math.inf),
            "width": (0.4, 6.1, 0.0, -1.0, math.nan, math.inf),
            "ghost_alpha": (-0.1, 1.1, math.nan, math.inf),
        }
        for name, values in cases.items():
            for value in values:
                with self.assertRaises(ValueError, msg="%s=%r" % (name, value)):
                    setattr(self.axis, name, value)

        self.assertEqual(self.axis.opacity, 0.65)
        self.assertEqual(self.axis.length, 80.0)
        self.assertEqual(self.axis.width, 1.5)
        self.assertAlmostEqual(self.axis.ghost_alpha, 0.10)

    def test_apply_builds_the_exact_axis_view(self):
        self.axis.enabled = True
        self.axis.color_x = "#f87171"
        self.axis.color_y = "#4ade80"
        self.axis.color_z = "#38bdf8"
        self.axis.opacity = 0.65
        self.axis.length = 80.0
        self.axis.width = 1.5
        self.axis.origin = True
        self.axis.labels = True
        self.axis.ghost_alpha = 0.10

        self.assertEqual(
            self.axis.apply(),
            {
                "view": {
                    "axis": {
                        "enabled": True,
                        "color_x": "#f87171",
                        "color_y": "#4ade80",
                        "color_z": "#38bdf8",
                        "opacity": 0.65,
                        "length": 80.0,
                        "width": 1.5,
                        "origin": True,
                        "labels": True,
                        "ghost_alpha": 0.10,
                    }
                }
            },
        )
        self.assertEqual(set(self.axis.apply()["view"]["axis"]), VIEW_KEYS)

    def test_snapshot_matches_the_axis_view(self):
        self.assertEqual(FreeCAD.GuiUp, 0)
        snapshot = self.axis.snapshot()
        self.assertEqual(set(snapshot), VIEW_KEYS)
        self.assertEqual(snapshot, self.axis.apply()["view"]["axis"])

        self.axis.length = 240.0
        self.axis.color_x = "#00ff00"
        snapshot = self.axis.snapshot()
        self.assertEqual(snapshot["length"], 240.0)
        self.assertEqual(snapshot["color_x"], "#00ff00")

    def test_apply_does_not_raise_headless(self):
        self.assertEqual(FreeCAD.GuiUp, 0, "this suite runs headless")
        self.assertIsInstance(self.axis.apply()["view"]["axis"], dict)

    def test_reset_clears_the_axis_group_only(self):
        self.axis.enabled = False
        self.axis.color_x = "#000000"
        self.axis.length = 20.0
        grid = FreeCAD.ParamGet(GRID_PREFERENCES)
        grid.SetBool("GridAxisCross", False)

        self.axis.reset()

        self.assertEqual(self.prefs.GetBools(), [])
        self.assertEqual(self.prefs.GetStrings(), [])
        self.assertEqual(self.prefs.GetFloats(), [])
        self.assertFalse(
            grid.GetBool("GridAxisCross", True),
            "reset never touches another provider's group",
        )
        # Drop the legacy flag so the post-reset read shows the defaults.
        grid.RemBool("GridAxisCross")
        self.assertTrue(self.axis.enabled)
        self.assertEqual(self.axis.color_x, "#f87171")
        self.assertEqual(self.axis.length, 80.0)

    def test_the_legacy_grid_provider_flag_migrates_once(self):
        grid = FreeCAD.ParamGet(GRID_PREFERENCES)
        grid.SetBool("GridAxisCross", False)

        self.assertFalse(self.axis.enabled)
        self.assertEqual(
            self.prefs.GetBools(),
            ["enabled"],
            "the first read copies the legacy value into the axis group",
        )
        self.assertFalse(self.prefs.GetBool("enabled", True))

        # The axis group is authoritative once migrated.
        grid.SetBool("GridAxisCross", True)
        self.assertFalse(self.axis.enabled)

    def test_the_legacy_view_flag_is_the_second_source(self):
        view = FreeCAD.ParamGet(LEGACY_PREFERENCES)
        view.SetBool("GridAxisCross", False)

        self.assertFalse(self.axis.enabled)
        self.assertFalse(self.prefs.GetBool("enabled", True))

        view.SetBool("GridAxisCross", True)
        self.assertFalse(self.axis.enabled, "read once, not on every read")

    def test_the_grid_provider_flag_wins_over_the_view_flag(self):
        FreeCAD.ParamGet(GRID_PREFERENCES).SetBool("GridAxisCross", True)
        FreeCAD.ParamGet(LEGACY_PREFERENCES).SetBool("GridAxisCross", False)

        self.assertTrue(self.axis.enabled)
        self.assertTrue(self.prefs.GetBool("enabled", False))

    def test_migration_touches_no_other_key(self):
        view = FreeCAD.ParamGet(LEGACY_PREFERENCES)
        view.SetBool("SomeOtherPreference", True)

        self.assertTrue(self.axis.enabled)
        self.assertTrue(view.GetBool("SomeOtherPreference", False))
        self.assertEqual(
            self.prefs.GetBools(),
            ["enabled"],
            "only the enabled marker is written",
        )
        view.RemBool("SomeOtherPreference")

    def test_the_grid_property_is_a_deprecated_proxy(self):
        self.axis.enabled = False
        self.assertFalse(FerroCAD.Grid.axis_cross)

        FerroCAD.Grid.axis_cross = True
        self.assertTrue(self.axis.enabled)
        self.assertIn("Deprecated", type(FerroCAD.Grid).axis_cross.__doc__ or "")


class AxisCrossCommandTests(unittest.TestCase):
    """``App_ToggleAxisCross`` writes ``FerroCAD.Axis.enabled``."""

    def setUp(self):
        self.axis = FerroCAD.Axis
        self.axis.reset()
        clear_preferences()
        FerroCADGui.ViewState.axis_cross = True
        for provider in FerroCADGui.providers("axis"):
            FerroCADGui.unregisterProvider("axis", provider["backend"])

    def tearDown(self):
        for provider in FerroCADGui.providers("axis"):
            FerroCADGui.unregisterProvider("axis", provider["backend"])
        self.axis.reset()
        clear_preferences()

    def test_the_command_flips_enabled_and_returns_the_axis_view(self):
        self.axis.enabled = False

        request = FreeCADGui.runCommand("App_ToggleAxisCross")

        self.assertTrue(self.axis.enabled)
        self.assertEqual(request, self.axis.apply())
        self.assertTrue(request["view"]["axis"]["enabled"])

        request = FreeCADGui.runCommand("App_ToggleAxisCross")
        self.assertFalse(self.axis.enabled)
        self.assertFalse(request["view"]["axis"]["enabled"])

    def test_the_command_keeps_the_upstream_request_without_the_axis_api(self):
        FerroCADGui.ViewState.axis_cross = True
        with mock.patch.object(FerroCAD, "Axis", None):
            request = FreeCADGui.runCommand("App_ToggleAxisCross")
        self.assertEqual(request, {"view": {"axis_cross": False}})

    def test_without_a_provider_the_toggle_makes_no_overlay_claim(self):
        self.axis.enabled = False

        request = FreeCADGui.runCommand("App_ToggleAxisCross")

        self.assertTrue(request["view"]["axis"]["enabled"])
        self.assertNotIn("overlay", request["view"]["axis"])

    def test_the_toggle_merges_the_native_backends_no_overlay_claim(self):
        FerroCADGui.registerProvider(
            "axis",
            "ferrocad",
            "FerroCAD (native)",
            priority=10,
            refresh=lambda: {"view": {"axis": {"overlay": False}}},
        )
        self.axis.enabled = False

        request = FreeCADGui.runCommand("App_ToggleAxisCross")

        self.assertTrue(self.axis.enabled)
        self.assertIs(request["view"]["axis"]["overlay"], False)

    def test_the_toggle_merges_the_coin_backends_overlay_claim(self):
        FerroCADGui.registerProvider(
            "axis",
            "coin",
            "Coin (WGSL)",
            priority=0,
            refresh=lambda: {"view": {"axis": {"overlay": True}}},
        )
        self.axis.enabled = False

        request = FreeCADGui.runCommand("App_ToggleAxisCross")

        self.assertTrue(self.axis.enabled)
        self.assertIs(request["view"]["axis"]["overlay"], True)

        # The claim follows the master switch: turning the axis off restores
        # the native layer instead of hiding it for a disabled overlay.
        request = FreeCADGui.runCommand("App_ToggleAxisCross")
        self.assertFalse(self.axis.enabled)
        self.assertIs(request["view"]["axis"]["overlay"], False)


class AxisSummaryTests(unittest.TestCase):
    """``axis_summary()`` is the shell's read-only provider view."""

    def setUp(self):
        self.axis = FerroCAD.Axis
        self.axis.reset()
        clear_preferences()
        for provider in FerroCADGui.providers("axis"):
            FerroCADGui.unregisterProvider("axis", provider["backend"])

    def tearDown(self):
        for provider in FerroCADGui.providers("axis"):
            FerroCADGui.unregisterProvider("axis", provider["backend"])
        self.axis.reset()
        clear_preferences()

    def register_native(self):
        FerroCADGui.registerProvider(
            "axis",
            "ferrocad",
            "FerroCAD (native)",
            priority=10,
            settings=_axis.AXIS_SETTINGS_SCHEMA,
        )

    def register_coin(self):
        FerroCADGui.registerProvider("axis", "coin", "Coin (WGSL)", priority=0)

    def test_without_a_provider_the_summary_keeps_the_closed_shape(self):
        self.assertEqual(
            json.loads(ferrocad_shell.axis_summary()),
            {
                "feature": "axis",
                "backend": None,
                "label": None,
                "length": None,
                "width": None,
                "providers": [],
            },
        )

    def test_the_summary_names_the_active_provider_and_stem_size(self):
        self.register_native()
        self.axis.length = 80.0
        self.axis.width = 1.5
        summary = json.loads(ferrocad_shell.axis_summary())
        self.assertEqual(summary["feature"], "axis")
        self.assertEqual(summary["backend"], "ferrocad")
        self.assertEqual(summary["label"], "FerroCAD (native)")
        self.assertEqual(summary["length"], "80.0 px")
        self.assertEqual(summary["width"], "1.5 px")
        self.assertEqual(
            summary["providers"],
            [
                {
                    "backend": "ferrocad",
                    "label": "FerroCAD (native)",
                    "active": True,
                }
            ],
        )

        self.register_coin()
        FerroCADGui.setActiveProvider("axis", "coin")
        summary = json.loads(ferrocad_shell.axis_summary())
        self.assertEqual(summary["backend"], "coin")
        self.assertEqual(summary["label"], "Coin (WGSL)")
        self.assertEqual(
            summary["length"],
            "80 px",
            "a backend without a stem row reports the one axis's size",
        )
        self.assertEqual(summary["width"], "1.5 px")
        self.assertEqual(
            [provider["backend"] for provider in summary["providers"]],
            ["ferrocad", "coin"],
        )
        self.assertEqual(
            [provider["active"] for provider in summary["providers"]],
            [False, True],
        )


class AxisSettingsCommandTests(unittest.TestCase):
    """``View_AxisSettings`` is the axis popover's one settings row."""

    COMMAND = "View_AxisSettings"

    def setUp(self):
        for provider in FerroCADGui.providers("axis"):
            FerroCADGui.unregisterProvider("axis", provider["backend"])

    def tearDown(self):
        for provider in FerroCADGui.providers("axis"):
            FerroCADGui.unregisterProvider("axis", provider["backend"])

    def test_the_command_registers_on_the_axis_surface(self):
        command = next(
            item
            for item in json.loads(ferrocad_shell.commands())
            if item["id"] == self.COMMAND
        )
        self.assertEqual(command["menu"], "Axis settings")
        self.assertEqual(command["tooltip"], "Edit the axis cross")
        self.assertEqual(command["group"], "View")
        self.assertEqual(command["cmd_type"], "ForView")
        self.assertEqual(command["surface"], "axis")
        self.assertEqual(command["parent"], "App_ToggleAxisCross")
        self.assertFalse(command["dev_only"])

    def test_activated_selects_the_axis_settings_node_with_a_provider(self):
        FerroCADGui.registerProvider("axis", "ferrocad", "FerroCAD (native)")
        self.assertEqual(
            FreeCADGui.runCommand(self.COMMAND), {"select": "Settings/Axis"}
        )

    def test_activated_reports_without_a_provider(self):
        self.assertEqual(
            FreeCADGui.runCommand(self.COMMAND),
            {"status": "no axis provider is registered"},
        )


class SetProviderTests(unittest.TestCase):
    """``ferrocad_shell.set_provider`` is the popover's switch bridge."""

    def setUp(self):
        for provider in FerroCADGui.providers("axis"):
            FerroCADGui.unregisterProvider("axis", provider["backend"])

    def tearDown(self):
        for provider in FerroCADGui.providers("axis"):
            FerroCADGui.unregisterProvider("axis", provider["backend"])

    def register_native(self):
        FerroCADGui.registerProvider(
            "axis",
            "ferrocad",
            "FerroCAD (native)",
            priority=10,
            activate=lambda: {"view": {"axis": {"overlay": False}}},
            deactivate=lambda: {},
        )

    def register_coin(self):
        FerroCADGui.registerProvider(
            "axis",
            "coin",
            "Coin (WGSL)",
            priority=0,
            activate=lambda: {"view": {"axis": {"overlay": True}}},
        )

    def test_a_valid_switch_returns_the_status_and_the_merged_view(self):
        self.register_native()
        self.register_coin()
        outcome = json.loads(
            ferrocad_shell.set_provider(
                json.dumps({"feature": "axis", "backend": "coin"})
            )
        )
        self.assertTrue(outcome["ok"], outcome)
        self.assertEqual(outcome["error"], "")
        self.assertIn("axis", outcome["status"])
        self.assertIn("coin", outcome["status"])
        self.assertTrue(outcome["view"]["axis"]["overlay"])
        self.assertEqual(FerroCADGui.activeProvider("axis"), "coin")

    def test_an_unknown_feature_or_backend_is_reported_and_never_raised(self):
        self.register_native()
        for payload in (
            '{"feature": "nope", "backend": "ferrocad"}',
            '{"feature": "axis", "backend": "nope"}',
            "not json",
            "null",
        ):
            with self.subTest(payload=payload):
                outcome = json.loads(ferrocad_shell.set_provider(payload))
                self.assertFalse(outcome["ok"], outcome)
                self.assertTrue(outcome["error"], outcome)
                self.assertIn("status", outcome)
                self.assertIn("view", outcome)


_LOADED = None


def load_mod():
    """Load ``mods/AxisOverlay/InitGui.py`` once, by path."""
    global _LOADED
    if _LOADED is not None:
        return _LOADED
    FreeCADGui.removeCommand(COMMAND)
    spec = importlib.util.spec_from_file_location("AxisOverlay_InitGui", MOD_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    _LOADED = module
    return module


class AxisOverlayModTests(unittest.TestCase):
    def setUp(self):
        self.axis = FerroCAD.Axis
        self.axis.reset()
        clear_preferences()
        for provider in FerroCADGui.providers("axis"):
            FerroCADGui.unregisterProvider("axis", provider["backend"])
        self.mod = load_mod()
        self.mod.register_provider()
        FerroCADGui.setActiveProvider("axis", "ferrocad")

    def tearDown(self):
        FerroCADGui.unregisterProvider("axis", "ferrocad")
        FreeCADGui._cancel_all()
        self.axis.reset()
        clear_preferences()

    def spec(self):
        return FreeCADGui.runCommand(COMMAND)

    def test_the_command_registers_with_the_axis_surface(self):
        self.assertIn(COMMAND, FreeCADGui.listCommands())
        entry = next(
            item for item in FreeCADGui._command_list() if item["id"] == COMMAND
        )
        self.assertEqual(entry["menu"], "Axis Cross")
        self.assertTrue(entry["enabled"], "IsActive() returns True")

        resources = self.mod.AxisOverlayCommand().GetResources()
        self.assertEqual(resources["FerroCAD_Surface"], "axis")
        self.assertEqual(resources["FerroCAD_Parent"], "App_ToggleAxisCross")
        self.assertEqual(resources["FerroCAD_DevOnly"], "true")
        self.assertEqual(resources["Group"], "View")
        self.assertEqual(resources["CmdType"], "ForView")
        self.assertTrue(entry["dev_only"], "the settings dialog lives in Dev Tools")

    def test_the_provider_advertises_the_canonical_schema(self):
        self.assertEqual(FerroCADGui.activeProvider("axis"), "ferrocad")
        self.assertEqual(
            [row["name"] for row in FerroCADGui.settings("axis")], list(ATTRIBUTES)
        )
        self.assertEqual(self.mod.settings_node(), "Settings/Axis")
        self.assertEqual(self.spec(), {"select": "Settings/Axis"})

    def test_settings_axis_is_in_the_tree_and_resolves_rows(self):
        tree = json.loads(ferrocad_shell.model_tree())
        self.assertEqual(tree[0]["name"], "Settings")
        self.assertIn(
            {"name": "Settings/axis", "label": "Axis", "type": "FerroCAD::Settings"},
            tree[0]["objects"],
        )

        rows = json.loads(ferrocad_shell.properties("Settings/Axis"))
        self.assertEqual(rows[0]["name"], "provider")
        self.assertEqual(rows[0]["value"], "FerroCAD (native)")
        self.assertEqual(
            [row["name"] for row in rows[1:]], list(ATTRIBUTES)
        )
        by_name = {row["name"]: row for row in rows[1:]}
        self.assertEqual(by_name["enabled"]["value"], "True")
        self.assertEqual(by_name["color_x"]["value"], "#f87171")
        self.assertEqual(by_name["length"]["value"], "80.0")
        self.assertEqual(by_name["ghost_alpha"]["value"], "0.1")

    def test_activate_and_deactivate_toggle_enabled(self):
        self.axis.enabled = False
        result = self.mod.activate()
        self.assertTrue(self.axis.enabled)
        expected = self.axis.apply()
        expected["view"]["axis"]["overlay"] = False
        self.assertEqual(result, expected)

        result = self.mod.deactivate()
        self.assertFalse(self.axis.enabled)
        expected = self.axis.apply()
        expected["view"]["axis"]["overlay"] = False
        self.assertEqual(result, expected)

    def test_the_native_backend_never_claims_the_overlay(self):
        self.axis.enabled = True
        self.assertIs(self.mod.refresh()["view"]["axis"]["overlay"], False)
        self.assertIs(self.mod.activate()["view"]["axis"]["overlay"], False)
        self.assertIs(self.mod.deactivate()["view"]["axis"]["overlay"], False)

    def test_the_reload_command_returns_the_axis_view_and_its_claim(self):
        entry = next(
            item
            for item in FreeCADGui._command_list()
            if item["id"] == "App_ReloadAxisSettings"
        )
        self.assertEqual(entry["menu"], "Reload axis settings")
        resources = ferrocad_shell._ReloadAxisSettingsCommand().GetResources()
        self.assertEqual(resources["MenuText"], "Reload axis settings")
        self.assertEqual(resources["Group"], "View")
        self.assertEqual(resources["CmdType"], "ForView")
        self.assertEqual(resources["FerroCAD_Surface"], "axis")
        self.assertEqual(resources["FerroCAD_Parent"], "App_ToggleAxisCross")

        self.axis.length = 120.0
        request = FreeCADGui.runCommand("App_ReloadAxisSettings")
        self.assertTrue(request["view"]["axis"]["enabled"])
        self.assertEqual(request["view"]["axis"]["length"], 120.0)
        self.assertIs(request["view"]["axis"]["overlay"], False)

    def test_register_provider_is_re_callable(self):
        self.mod.register_provider()
        self.assertEqual(
            [provider["backend"] for provider in FerroCADGui.providers("axis")],
            ["ferrocad"],
        )

    def test_the_fallback_dialog_covers_the_schema_and_writes_the_axis(self):
        FerroCADGui.unregisterProvider("axis", "ferrocad")
        try:
            spec = self.spec()
        finally:
            self.mod.register_provider()

        self.assertNotIn("select", spec)
        self.assertEqual(spec["title"], "Axis Cross Settings")
        self.assertTrue(callable(spec["on_accept"]))
        fields = {field["name"]: field for field in spec["fields"]}
        self.assertEqual(set(fields), set(ATTRIBUTES))
        self.assertEqual(fields["enabled"]["kind"], "bool")
        self.assertEqual(fields["color_x"]["kind"], "text")
        self.assertEqual(fields["length"]["kind"], "float")
        self.assertEqual(fields["length"]["min"], 8.0)
        self.assertEqual(fields["length"]["max"], 400.0)

        result = spec["on_accept"](
            {
                "enabled": False,
                "color_x": "#FF0000",
                "opacity": 0.5,
                "length": 120.0,
                "width": 2.5,
                "ghost_alpha": 0.2,
            }
        )
        self.assertEqual(result, self.axis.apply())
        self.assertFalse(self.axis.enabled)
        self.assertEqual(self.axis.color_x, "#ff0000")
        self.assertEqual(self.axis.opacity, 0.5)
        self.assertEqual(self.axis.length, 120.0)
        self.assertEqual(self.axis.width, 2.5)
        self.assertEqual(self.axis.ghost_alpha, 0.2)

    def test_a_bad_fallback_value_keeps_the_store_alone(self):
        FerroCADGui.unregisterProvider("axis", "ferrocad")
        try:
            spec = self.spec()
        finally:
            self.mod.register_provider()
        with self.assertRaises(ValueError):
            spec["on_accept"]({"color_x": "not a colour"})
        self.assertEqual(self.axis.color_x, "#f87171")

    def test_the_fallback_dialog_reports_without_the_axis_api(self):
        FerroCADGui.unregisterProvider("axis", "ferrocad")
        try:
            with mock.patch.object(FerroCAD, "Axis", None):
                spec = self.spec()
        finally:
            self.mod.register_provider()
        self.assertEqual(spec, {"status": self.mod.UPSTREAM_STATUS})


if __name__ == "__main__":
    unittest.main()
