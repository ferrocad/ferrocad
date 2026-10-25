"""Tests for the semantic gizmo API, ``FerroCAD.Gizmo``.

The gizmo capability mirrors the grid and axis ones: ``FerroCAD._gizmo`` owns
its own schema and parameter group, ``mods/GizmoOverlay`` advertises that schema
as the native ``gizmo``/``ferrocad`` provider, and ``apply()`` returns the
shell's ``{"view": {"gizmo": …}}`` request. The suite is headless
(``FreeCAD.GuiUp == 0``): every setter validates and stores, ``apply()`` returns
the request and never raises, and ``snapshot()`` reads the store back.
"""

import importlib.util
import io
import json
import math
import pathlib
import unittest
from unittest import mock

import FreeCAD
import FreeCADGui
import FerroCAD
import FerroCADGui
import ferrocad_shell  # noqa: F401  (registers the shell's commands)
from FerroCAD import _gizmo

PREFERENCES = "User parameter:BaseApp/Preferences/Mods/gizmo/ferrocad"
AXIS_PREFERENCES = "User parameter:BaseApp/Preferences/Mods/axis/ferrocad"

MOD_PATH = (
    pathlib.Path(__file__).resolve().parents[1]
    / "mods"
    / "GizmoOverlay"
    / "InitGui.py"
)
COMMAND = "GizmoOverlay_Settings"

SHADER_MOD_PATH = (
    pathlib.Path(__file__).resolve().parents[1]
    / "mods"
    / "GizmoOverlayShader"
    / "InitGui.py"
)
SHADER_COMMAND = "GizmoOverlayShader_Settings"

#: The canonical schema, in order, with each default.
ATTRIBUTES = {
    "enabled": True,
    "active_color": "#38bdf8",
    "inactive_color": "#94a3b8",
    "size": 80.0,
    "width": 1.5,
    "opacity": 1.0,
    "ghost_alpha": 0.35,
    "ring_snap_deg": 15.0,
    "show_readout": True,
}

VIEW_KEYS = set(ATTRIBUTES)
PAYLOAD_KEYS = VIEW_KEYS | {"backend"}


def clear_preferences():
    FreeCAD.ParamGet(PREFERENCES).Clear()


def clear_providers():
    for provider in FerroCADGui.providers("gizmo"):
        FerroCADGui.unregisterProvider("gizmo", provider["backend"])


class GizmoApiTests(unittest.TestCase):
    def setUp(self):
        self.gizmo = FerroCAD.Gizmo
        self.gizmo.reset()
        clear_preferences()
        clear_providers()
        self.prefs = FreeCAD.ParamGet(PREFERENCES)

    def tearDown(self):
        self.gizmo.reset()
        clear_preferences()
        clear_providers()

    def test_the_singleton_is_lazy_and_stable(self):
        self.assertIs(FerroCAD.Gizmo, self.gizmo)
        self.assertIs(FerroCAD.Gizmo, getattr(FerroCAD, "Gizmo"))
        self.assertIn("Gizmo", FerroCAD.__all__)

    def test_the_schema_is_the_shared_contract(self):
        self.assertEqual(
            [entry["name"] for entry in _gizmo.GIZMO_SETTINGS_SCHEMA],
            list(ATTRIBUTES),
        )
        self.assertEqual(
            [(entry["name"], entry["default"]) for entry in _gizmo.SETTINGS_SCHEMA],
            list(ATTRIBUTES.items()),
        )
        self.assertIs(_gizmo.GIZMO_SETTINGS_SCHEMA, _gizmo.SETTINGS_SCHEMA)
        kinds = {
            entry["name"]: entry["kind"] for entry in _gizmo.GIZMO_SETTINGS_SCHEMA
        }
        self.assertEqual(
            kinds,
            {
                "enabled": "bool",
                "active_color": "color",
                "inactive_color": "color",
                "size": "float",
                "width": "float",
                "opacity": "float",
                "ghost_alpha": "float",
                "ring_snap_deg": "float",
                "show_readout": "bool",
            },
        )
        bounds = {
            entry["name"]: (entry.get("min"), entry.get("max"))
            for entry in _gizmo.GIZMO_SETTINGS_SCHEMA
            if entry["kind"] == "float"
        }
        self.assertEqual(
            bounds,
            {
                "size": (8.0, 400.0),
                "width": (0.5, 6.0),
                "opacity": (0.0, 1.0),
                "ghost_alpha": (0.0, 1.0),
                "ring_snap_deg": (0.0, 180.0),
            },
        )

        # ``schema()`` is a copy: a caller cannot mutate the canonical list.
        schema = self.gizmo.schema()
        schema[0]["name"] = "mutated"
        self.assertEqual(_gizmo.GIZMO_SETTINGS_SCHEMA[0]["name"], "enabled")

    def test_the_defaults_are_the_contract_defaults(self):
        self.assertTrue(self.gizmo.enabled)
        self.assertEqual(self.gizmo.active_color, "#38bdf8")
        self.assertEqual(self.gizmo.inactive_color, "#94a3b8")
        self.assertEqual(self.gizmo.size, 80.0)
        self.assertEqual(self.gizmo.width, 1.5)
        self.assertEqual(self.gizmo.opacity, 1.0)
        self.assertEqual(self.gizmo.ghost_alpha, 0.35)
        self.assertEqual(self.gizmo.ring_snap_deg, 15.0)
        self.assertTrue(self.gizmo.show_readout)

    def test_the_store_uses_the_schema_names(self):
        self.gizmo.enabled = False
        self.gizmo.active_color = "#FF0000"
        self.gizmo.inactive_color = "#0f0"
        self.gizmo.size = 120.0
        self.gizmo.width = 3.5
        self.gizmo.opacity = 0.25
        self.gizmo.ghost_alpha = 0.4
        self.gizmo.ring_snap_deg = 45.0
        self.gizmo.show_readout = False

        self.assertFalse(self.prefs.GetBool("enabled", True))
        self.assertEqual(self.prefs.GetString("active_color", ""), "#ff0000")
        self.assertEqual(self.prefs.GetString("inactive_color", ""), "#00ff00")
        self.assertAlmostEqual(self.prefs.GetFloat("size", -1.0), 120.0)
        self.assertAlmostEqual(self.prefs.GetFloat("width", -1.0), 3.5)
        self.assertAlmostEqual(self.prefs.GetFloat("opacity", -1.0), 0.25)
        self.assertAlmostEqual(self.prefs.GetFloat("ghost_alpha", -1.0), 0.4)
        self.assertAlmostEqual(self.prefs.GetFloat("ring_snap_deg", -1.0), 45.0)
        self.assertFalse(self.prefs.GetBool("show_readout", True))

    def test_each_property_round_trips(self):
        self.gizmo.enabled = False
        self.assertFalse(self.gizmo.enabled)

        self.gizmo.active_color = "#F00"
        self.assertEqual(self.gizmo.active_color, "#ff0000")
        self.gizmo.inactive_color = "#0F0"
        self.assertEqual(self.gizmo.inactive_color, "#00ff00")

        for name, value in (
            ("size", 8.0),
            ("size", 400.0),
            ("width", 0.5),
            ("width", 6.0),
            ("opacity", 0.0),
            ("opacity", 1.0),
            ("ghost_alpha", 0.0),
            ("ghost_alpha", 1.0),
            ("ring_snap_deg", 0.0),
            ("ring_snap_deg", 180.0),
        ):
            setattr(self.gizmo, name, value)
            self.assertEqual(getattr(self.gizmo, name), value)

        self.gizmo.show_readout = False
        self.assertFalse(self.gizmo.show_readout)

    def test_bools_are_coerced(self):
        self.gizmo.enabled = 0
        self.assertIs(self.gizmo.enabled, False)
        self.gizmo.show_readout = "yes"
        self.assertIs(self.gizmo.show_readout, True)

    def test_a_bad_colour_raises_and_leaves_the_store_untouched(self):
        for bad in ("", "red", "#12", "#1234", "#1234567", "#gggggg", "#12345g", 42, None):
            for name in ("active_color", "inactive_color"):
                with self.assertRaises(ValueError, msg="%s=%r" % (name, bad)):
                    setattr(self.gizmo, name, bad)
        self.assertEqual(
            (self.gizmo.active_color, self.gizmo.inactive_color),
            ("#38bdf8", "#94a3b8"),
        )

    def test_bad_numbers_raise_and_leave_the_store_untouched(self):
        cases = {
            "size": (7.9, 400.1, 0.0, -1.0, math.nan, math.inf, -math.inf),
            "width": (0.4, 6.1, 0.0, -1.0, math.nan, math.inf),
            "opacity": (-0.1, 1.1, math.nan, math.inf),
            "ghost_alpha": (-0.1, 1.1, math.nan, math.inf),
            "ring_snap_deg": (-0.1, 180.1, math.nan, math.inf),
        }
        for name, values in cases.items():
            for value in values:
                with self.assertRaises(ValueError, msg="%s=%r" % (name, value)):
                    setattr(self.gizmo, name, value)

        self.assertEqual(self.gizmo.size, 80.0)
        self.assertEqual(self.gizmo.width, 1.5)
        self.assertEqual(self.gizmo.opacity, 1.0)
        self.assertEqual(self.gizmo.ghost_alpha, 0.35)
        self.assertEqual(self.gizmo.ring_snap_deg, 15.0)

    def test_apply_builds_the_exact_gizmo_view(self):
        self.gizmo.enabled = True
        self.gizmo.active_color = "#38bdf8"
        self.gizmo.inactive_color = "#94a3b8"
        self.gizmo.size = 80.0
        self.gizmo.width = 1.5
        self.gizmo.opacity = 1.0
        self.gizmo.ghost_alpha = 0.35
        self.gizmo.ring_snap_deg = 15.0
        self.gizmo.show_readout = True

        self.assertEqual(
            self.gizmo.apply(),
            {
                "view": {
                    "gizmo": {
                        "enabled": True,
                        "backend": "ferrocad",
                        "active_color": "#38bdf8",
                        "inactive_color": "#94a3b8",
                        "size": 80.0,
                        "width": 1.5,
                        "opacity": 1.0,
                        "ghost_alpha": 0.35,
                        "ring_snap_deg": 15.0,
                        "show_readout": True,
                    }
                }
            },
        )
        self.assertEqual(set(self.gizmo.apply()["view"]["gizmo"]), PAYLOAD_KEYS)

    def test_apply_names_the_active_provider(self):
        FerroCADGui.registerProvider("gizmo", "other", "Other backend")
        try:
            FerroCADGui.setActiveProvider("gizmo", "other")
            self.assertEqual(
                self.gizmo.apply()["view"]["gizmo"]["backend"], "other"
            )
        finally:
            FerroCADGui.unregisterProvider("gizmo", "other")
        self.assertEqual(
            self.gizmo.apply()["view"]["gizmo"]["backend"], "ferrocad"
        )

    def test_snapshot_matches_the_gizmo_view_without_the_backend(self):
        self.assertEqual(FreeCAD.GuiUp, 0)
        snapshot = self.gizmo.snapshot()
        self.assertEqual(set(snapshot), VIEW_KEYS)
        self.assertEqual(
            snapshot,
            {
                key: value
                for key, value in self.gizmo.apply()["view"]["gizmo"].items()
                if key != "backend"
            },
        )

        self.gizmo.size = 240.0
        self.gizmo.active_color = "#00ff00"
        snapshot = self.gizmo.snapshot()
        self.assertEqual(snapshot["size"], 240.0)
        self.assertEqual(snapshot["active_color"], "#00ff00")

    def test_apply_does_not_raise_headless(self):
        self.assertEqual(FreeCAD.GuiUp, 0, "this suite runs headless")
        self.assertIsInstance(self.gizmo.apply()["view"]["gizmo"], dict)

    def test_reset_clears_the_gizmo_group_only(self):
        self.gizmo.enabled = False
        self.gizmo.active_color = "#000000"
        self.gizmo.size = 20.0
        axis = FreeCAD.ParamGet(AXIS_PREFERENCES)
        axis.SetBool("enabled", False)

        self.gizmo.reset()

        self.assertEqual(self.prefs.GetBools(), [])
        self.assertEqual(self.prefs.GetStrings(), [])
        self.assertEqual(self.prefs.GetFloats(), [])
        self.assertFalse(
            axis.GetBool("enabled", True),
            "reset never touches another provider's group",
        )
        self.assertTrue(self.gizmo.enabled)
        self.assertEqual(self.gizmo.active_color, "#38bdf8")
        self.assertEqual(self.gizmo.size, 80.0)
        axis.RemBool("enabled")


_LOADED = None


def load_mod():
    """Load ``mods/GizmoOverlay/InitGui.py`` once, by path."""
    global _LOADED
    if _LOADED is not None:
        return _LOADED
    FreeCADGui.removeCommand(COMMAND)
    spec = importlib.util.spec_from_file_location("GizmoOverlay_InitGui", MOD_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    _LOADED = module
    return module


_SHADER_LOADED = None


def exec_shader_mod():
    """Execute ``mods/GizmoOverlayShader/InitGui.py`` afresh, by path."""
    FreeCADGui.removeCommand(SHADER_COMMAND)
    spec = importlib.util.spec_from_file_location(
        "GizmoOverlayShader_InitGui", SHADER_MOD_PATH
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def load_shader_mod():
    """Load ``mods/GizmoOverlayShader/InitGui.py`` once, by path."""
    global _SHADER_LOADED
    if _SHADER_LOADED is None:
        _SHADER_LOADED = exec_shader_mod()
    return _SHADER_LOADED


class GizmoOverlayModTests(unittest.TestCase):
    def setUp(self):
        self.gizmo = FerroCAD.Gizmo
        self.gizmo.reset()
        clear_preferences()
        clear_providers()
        self.mod = load_mod()
        self.mod.register_provider()
        FerroCADGui.setActiveProvider("gizmo", "ferrocad")

    def tearDown(self):
        clear_providers()
        FreeCADGui._cancel_all()
        self.gizmo.reset()
        clear_preferences()

    def spec(self):
        return FreeCADGui.runCommand(COMMAND)

    def test_the_command_registers_on_the_gizmo_surface(self):
        self.assertTrue(self.mod.register_provider(), "register_provider is re-callable")
        self.assertIn(COMMAND, FreeCADGui.listCommands())
        resource = self.mod.GizmoOverlayCommand().GetResources()
        self.assertEqual(resource["FerroCAD_Surface"], "gizmo")
        self.assertEqual(resource["FerroCAD_Parent"], "")
        self.assertEqual(resource["FerroCAD_DevOnly"], "true")
        self.assertEqual(resource["Group"], "View")
        self.assertEqual(resource["CmdType"], "ForView")

        entry = next(
            item
            for item in json.loads(ferrocad_shell.commands())
            if item["id"] == COMMAND
        )
        self.assertEqual(entry["surface"], "gizmo")
        self.assertEqual(entry["parent"], "")
        self.assertEqual(entry["group"], "View")
        self.assertEqual(entry["cmd_type"], "ForView")
        self.assertTrue(entry["dev_only"])

    def test_the_provider_advertises_the_canonical_schema(self):
        self.assertEqual(FerroCADGui.activeProvider("gizmo"), "ferrocad")
        self.assertEqual(
            [row["name"] for row in FerroCADGui.settings("gizmo")],
            list(ATTRIBUTES),
        )
        providers = FerroCADGui.providers("gizmo")
        self.assertEqual([item["backend"] for item in providers], ["ferrocad"])
        self.assertEqual(providers[0]["label"], "FerroCAD (native)")
        self.assertEqual(providers[0]["command"], COMMAND)
        self.assertEqual(providers[0]["priority"], 10)
        self.assertEqual(self.mod.settings_node(), "Settings/Gizmo")
        self.assertEqual(self.spec(), {"select": "Settings/Gizmo"})

    def test_settings_gizmo_is_in_the_tree_and_resolves_rows(self):
        tree = json.loads(ferrocad_shell.model_tree())
        self.assertEqual(tree[0]["name"], "Settings")
        self.assertIn(
            {"name": "Settings/gizmo", "label": "Gizmo", "type": "FerroCAD::Settings"},
            tree[0]["objects"],
        )

        rows = json.loads(ferrocad_shell.properties("Settings/Gizmo"))
        self.assertEqual(rows[0]["name"], "provider")
        self.assertEqual(rows[0]["value"], "FerroCAD (native)")
        # An edition preset adds a trailing action row; the schema rows keep their order.
        schema = [row for row in rows[1:] if row["name"] != "reset"]
        self.assertEqual([row["name"] for row in schema], list(ATTRIBUTES))
        by_name = {row["name"]: row for row in schema}
        self.assertEqual(by_name["enabled"]["value"], "True")
        self.assertEqual(by_name["active_color"]["value"], "#38bdf8")
        self.assertEqual(by_name["size"]["value"], "80.0")
        self.assertEqual(by_name["ghost_alpha"]["value"], "0.35")
        self.assertEqual(by_name["ring_snap_deg"]["value"], "15.0")

    def test_activate_and_deactivate_toggle_enabled(self):
        self.gizmo.enabled = False
        result = self.mod.activate()
        self.assertTrue(self.gizmo.enabled)
        expected = self.gizmo.apply()
        expected["view"]["gizmo"]["backend"] = "ferrocad"
        self.assertEqual(result, expected)

        result = self.mod.deactivate()
        self.assertFalse(self.gizmo.enabled)
        expected = self.gizmo.apply()
        expected["view"]["gizmo"]["backend"] = "ferrocad"
        self.assertEqual(result, expected)

    def test_the_callbacks_always_name_the_native_backend(self):
        FerroCADGui.registerProvider("gizmo", "coin", "Coin", priority=20)
        try:
            FerroCADGui.setActiveProvider("gizmo", "coin")
            self.assertEqual(FerroCADGui.activeProvider("gizmo"), "coin")
            for callback in (self.mod.activate, self.mod.deactivate, self.mod.refresh):
                with self.subTest(callback=callback.__name__):
                    request = callback()
                    self.assertEqual(request["view"]["gizmo"]["backend"], "ferrocad")
        finally:
            FerroCADGui.unregisterProvider("gizmo", "coin")

    def test_registration_survives_a_reload(self):
        self.mod.register_provider()
        self.assertEqual(
            [provider["backend"] for provider in FerroCADGui.providers("gizmo")],
            ["ferrocad"],
        )

    def test_the_fallback_dialog_covers_the_schema_and_writes_the_gizmo(self):
        FerroCADGui.unregisterProvider("gizmo", "ferrocad")
        try:
            spec = self.spec()
        finally:
            self.mod.register_provider()

        self.assertNotIn("select", spec)
        self.assertEqual(spec["title"], "Gizmo Settings")
        self.assertEqual(spec["accept"], "Apply")
        self.assertTrue(callable(spec["on_accept"]))
        fields = {field["name"]: field for field in spec["fields"]}
        self.assertEqual(set(fields), set(ATTRIBUTES))
        self.assertEqual(fields["enabled"]["kind"], "bool")
        self.assertEqual(fields["active_color"]["kind"], "text")
        self.assertEqual(fields["size"]["kind"], "float")
        self.assertEqual(fields["size"]["min"], 8.0)
        self.assertEqual(fields["size"]["max"], 400.0)
        self.assertEqual(fields["ring_snap_deg"]["min"], 0.0)
        self.assertEqual(fields["ring_snap_deg"]["max"], 180.0)

        result = spec["on_accept"](
            {
                "enabled": False,
                "active_color": "#FF0000",
                "size": 120.0,
                "ring_snap_deg": 45.0,
                "show_readout": False,
            }
        )
        self.assertEqual(result, self.gizmo.apply())
        self.assertFalse(self.gizmo.enabled)
        self.assertEqual(self.gizmo.active_color, "#ff0000")
        self.assertEqual(self.gizmo.size, 120.0)
        self.assertEqual(self.gizmo.ring_snap_deg, 45.0)
        self.assertFalse(self.gizmo.show_readout)

    def test_a_bad_fallback_value_keeps_the_store_alone(self):
        FerroCADGui.unregisterProvider("gizmo", "ferrocad")
        try:
            spec = self.spec()
        finally:
            self.mod.register_provider()
        with self.assertRaises(ValueError):
            spec["on_accept"]({"active_color": "not a colour"})
        self.assertEqual(self.gizmo.active_color, "#38bdf8")
        with self.assertRaises(ValueError):
            spec["on_accept"]({"size": 401.0})
        self.assertEqual(self.gizmo.size, 80.0)

    def test_the_fallback_dialog_reports_without_the_gizmo_api(self):
        FerroCADGui.unregisterProvider("gizmo", "ferrocad")
        try:
            with mock.patch.object(FerroCAD, "Gizmo", None):
                spec = self.spec()
        finally:
            self.mod.register_provider()
        self.assertEqual(spec, {"status": self.mod.UPSTREAM_STATUS})


class GizmoOverlayShaderModTests(unittest.TestCase):
    """The compiled-skin backend, mirroring the native one."""

    def setUp(self):
        self.gizmo = FerroCAD.Gizmo
        self.gizmo.reset()
        clear_preferences()
        clear_providers()
        self.native = load_mod()
        self.native.register_provider()
        self.mod = load_shader_mod()
        self.mod.register_provider()
        FerroCADGui.setActiveProvider("gizmo", "ferrocad-shader")

    def tearDown(self):
        clear_providers()
        FreeCADGui._cancel_all()
        self.gizmo.reset()
        clear_preferences()

    def spec(self):
        return FreeCADGui.runCommand(SHADER_COMMAND)

    def test_the_command_registers_on_the_gizmo_surface(self):
        self.assertTrue(self.mod.register_provider(), "register_provider is re-callable")
        self.assertIn(SHADER_COMMAND, FreeCADGui.listCommands())
        resource = self.mod.GizmoOverlayShaderCommand().GetResources()
        self.assertEqual(resource["MenuText"], "Gizmo (shader)")
        self.assertEqual(resource["FerroCAD_Surface"], "gizmo")
        self.assertEqual(resource["FerroCAD_Parent"], "")
        self.assertEqual(resource["FerroCAD_DevOnly"], "true")
        self.assertEqual(resource["Group"], "View")
        self.assertEqual(resource["CmdType"], "ForView")

        entry = next(
            item
            for item in json.loads(ferrocad_shell.commands())
            if item["id"] == SHADER_COMMAND
        )
        self.assertEqual(entry["surface"], "gizmo")
        self.assertEqual(entry["parent"], "")
        self.assertEqual(entry["group"], "View")
        self.assertEqual(entry["cmd_type"], "ForView")
        self.assertTrue(entry["dev_only"])

    def test_the_provider_advertises_the_canonical_schema(self):
        self.assertEqual(FerroCADGui.activeProvider("gizmo"), "ferrocad-shader")
        self.assertEqual(
            [row["name"] for row in FerroCADGui.settings("gizmo")],
            list(ATTRIBUTES),
        )
        self.assertEqual(
            [row["name"] for row in FerroCADGui.settings("gizmo", "ferrocad-shader")],
            list(ATTRIBUTES),
        )
        manifest = next(
            provider
            for provider in FerroCADGui.providers("gizmo")
            if provider["backend"] == "ferrocad-shader"
        )
        self.assertEqual(manifest["label"], "FerroCAD (shader)")
        self.assertEqual(manifest["command"], SHADER_COMMAND)
        self.assertEqual(manifest["priority"], 0)
        self.assertTrue(manifest["active"])
        self.assertEqual(self.mod.settings_node(), "Settings/Gizmo")
        self.assertEqual(self.spec(), {"select": "Settings/Gizmo"})

    def test_activate_and_deactivate_toggle_enabled(self):
        self.gizmo.enabled = False
        result = self.mod.activate()
        self.assertTrue(self.gizmo.enabled)
        expected = self.gizmo.apply()
        expected["view"]["gizmo"]["backend"] = "ferrocad-shader"
        self.assertEqual(result, expected)
        self.assertEqual(result["view"]["gizmo"]["backend"], "ferrocad-shader")

        result = self.mod.deactivate()
        self.assertFalse(self.gizmo.enabled)
        expected = self.gizmo.apply()
        expected["view"]["gizmo"]["backend"] = "ferrocad-shader"
        self.assertEqual(result, expected)
        self.assertEqual(result["view"]["gizmo"]["backend"], "ferrocad-shader")

    def test_the_callbacks_always_name_the_shader_backend(self):
        FerroCADGui.setActiveProvider("gizmo", "ferrocad")
        self.assertEqual(FerroCADGui.activeProvider("gizmo"), "ferrocad")
        self.gizmo.enabled = True
        requests = {
            "activate": self.mod.activate(),
            "deactivate": self.mod.deactivate(),
            "refresh": self.mod.refresh(),
        }
        for name, request in requests.items():
            with self.subTest(callback=name):
                self.assertEqual(
                    request["view"]["gizmo"]["backend"], "ferrocad-shader"
                )

    def test_both_backends_coexist_with_the_native_row_first(self):
        providers = FerroCADGui.providers("gizmo")
        self.assertEqual(
            [provider["backend"] for provider in providers],
            ["ferrocad", "ferrocad-shader"],
        )
        self.assertEqual([provider["priority"] for provider in providers], [10, 0])
        # The native backend stays the default: a fresh registration must not
        # steal the selection from the priority-10 native row.
        self.assertEqual(providers[0]["label"], "FerroCAD (native)")
        self.assertEqual(providers[0]["command"], COMMAND)
        self.assertEqual(providers[1]["label"], "FerroCAD (shader)")

    def test_settings_gizmo_still_resolves(self):
        tree = json.loads(ferrocad_shell.model_tree())
        self.assertEqual(tree[0]["name"], "Settings")
        self.assertIn(
            {"name": "Settings/gizmo", "label": "Gizmo", "type": "FerroCAD::Settings"},
            tree[0]["objects"],
        )

        rows = json.loads(ferrocad_shell.properties("Settings/Gizmo"))
        self.assertEqual(rows[0]["name"], "provider")
        self.assertEqual(rows[0]["value"], "FerroCAD (shader)")
        schema = [row for row in rows[1:] if row["name"] != "reset"]
        self.assertEqual([row["name"] for row in schema], list(ATTRIBUTES))
        self.assertEqual(
            {row["name"]: row["value"] for row in schema}["size"], "80.0"
        )

    def test_reloading_init_gui_does_not_double_register(self):
        self.mod.register_provider()
        exec_shader_mod()
        self.assertEqual(
            [provider["backend"] for provider in FerroCADGui.providers("gizmo")],
            ["ferrocad", "ferrocad-shader"],
        )
        self.assertIn(SHADER_COMMAND, FreeCADGui.listCommands())
        self.assertEqual(
            [
                entry
                for entry in FreeCADGui.listCommands()
                if entry == SHADER_COMMAND
            ],
            [SHADER_COMMAND],
        )
        self.assertEqual(self.spec(), {"select": "Settings/Gizmo"})

    def test_the_mod_supplies_its_skin_through_the_registry(self):
        shipped = pathlib.Path(self.mod.WGSL_PATH).read_text(encoding="utf-8")
        source = FerroCADGui.providerSkin("gizmo", "ferrocad-shader")
        self.assertIsInstance(source, str)
        self.assertTrue(source, "the shader backend supplies a skin")
        # The registry stores the source trimmed; the file itself ends with a
        # newline.
        self.assertEqual(source, shipped.strip())

        # With no backend named, the host asks the active one: the native
        # backend has no skin, so it draws its own material.
        self.assertEqual(FerroCADGui.providerSkin("gizmo"), source)
        try:
            FerroCADGui.setActiveProvider("gizmo", "ferrocad")
            self.assertEqual(FerroCADGui.activeProvider("gizmo"), "ferrocad")
            self.assertIsNone(FerroCADGui.providerSkin("gizmo"))
            FerroCADGui.setActiveProvider("gizmo", "ferrocad-shader")
            self.assertEqual(FerroCADGui.activeProvider("gizmo"), "ferrocad-shader")
            self.assertEqual(FerroCADGui.providerSkin("gizmo"), source)
        finally:
            FerroCADGui.setActiveProvider("gizmo", "ferrocad-shader")

    def test_the_skin_is_the_shipped_geometry_program(self):
        source = self.mod.skin_source()
        self.assertIsNotNone(source)
        for marker in ("@vertex", "fn vs_main", "@group(0)", "@group(2)"):
            with self.subTest(marker=marker):
                self.assertIn(marker, source)

    def test_the_manifest_does_not_carry_the_skin(self):
        manifest = next(
            provider
            for provider in FerroCADGui.providers("gizmo")
            if provider["backend"] == "ferrocad-shader"
        )
        self.assertNotIn("skin", manifest, "a skin is not a setting")

    def test_a_missing_skin_registers_and_falls_back_to_native(self):
        module = self.mod
        loader = mock.Mock(side_effect=OSError("no such file"))
        stderr = io.StringIO()
        with mock.patch.object(module, "_read_skin", loader), \
                mock.patch.object(module, "_SKIN", None), \
                mock.patch.object(module, "_SKIN_READ", False), \
                mock.patch("sys.stderr", stderr):
            self.assertIsNone(module.skin_source())
            self.assertEqual(loader.call_count, 1)
            self.assertTrue(module.register_provider(), "the provider still registers")
            self.assertIsNone(
                module.skin_source(), "the failed read is cached, not retried"
            )
            self.assertEqual(loader.call_count, 1)
            self.assertIsNone(FerroCADGui.providerSkin("gizmo", "ferrocad-shader"))
            self.assertIsNone(FerroCADGui.providerSkin("gizmo"))

        report = stderr.getvalue().strip().splitlines()
        self.assertEqual(len(report), 1, "one line on stderr")
        self.assertIn(module.WGSL_PATH, report[0])

        # The real source is restored with the patched module globals.
        self.mod.register_provider()
        self.assertTrue(FerroCADGui.providerSkin("gizmo", "ferrocad-shader"))

    def test_the_fallback_dialog_covers_the_schema_and_writes_the_gizmo(self):
        clear_providers()
        try:
            spec = self.spec()
        finally:
            self.native.register_provider()
            self.mod.register_provider()

        self.assertNotIn("select", spec)
        self.assertEqual(spec["title"], "Gizmo Settings")
        self.assertEqual(spec["accept"], "Apply")
        self.assertTrue(callable(spec["on_accept"]))
        fields = {field["name"]: field for field in spec["fields"]}
        self.assertEqual(set(fields), set(ATTRIBUTES))
        self.assertEqual(fields["enabled"]["kind"], "bool")
        self.assertEqual(fields["active_color"]["kind"], "text")
        self.assertEqual(fields["size"]["kind"], "float")
        self.assertEqual(fields["size"]["min"], 8.0)
        self.assertEqual(fields["size"]["max"], 400.0)

        result = spec["on_accept"](
            {
                "enabled": False,
                "active_color": "#00FF00",
                "size": 96.0,
                "ring_snap_deg": 30.0,
                "show_readout": False,
            }
        )
        self.assertEqual(result, self.gizmo.apply())
        self.assertFalse(self.gizmo.enabled)
        self.assertEqual(self.gizmo.active_color, "#00ff00")
        self.assertEqual(self.gizmo.size, 96.0)
        self.assertEqual(self.gizmo.ring_snap_deg, 30.0)
        self.assertFalse(self.gizmo.show_readout)

    def test_a_bad_fallback_value_keeps_the_store_alone(self):
        clear_providers()
        try:
            spec = self.spec()
        finally:
            self.native.register_provider()
            self.mod.register_provider()
        with self.assertRaises(ValueError):
            spec["on_accept"]({"active_color": "not a colour"})
        self.assertEqual(self.gizmo.active_color, "#38bdf8")
        with self.assertRaises(ValueError):
            spec["on_accept"]({"size": 401.0})
        self.assertEqual(self.gizmo.size, 80.0)

    def test_the_fallback_dialog_reports_without_the_gizmo_api(self):
        clear_providers()
        try:
            with mock.patch.object(FerroCAD, "Gizmo", None):
                spec = self.spec()
        finally:
            self.native.register_provider()
            self.mod.register_provider()
        self.assertEqual(spec, {"status": self.mod.UPSTREAM_STATUS})


if __name__ == "__main__":
    unittest.main()
