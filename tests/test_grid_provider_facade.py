"""S4: ``FerroCAD.Grid`` is a facade over the native provider's settings store.

The old attribute API and ``Settings/Grid`` must be two views of one parameter
group (``User parameter:BaseApp/Preferences/Mods/grid/ferrocad``), a provider
switch must change the advertised rows, and the pre-S4 ``Preferences/View``
values must migrate once. The tests register stub providers rather than loading
both mods, because the settings model is what S4 changes; the native mod is
loaded once to prove its registration advertises the canonical schema.
"""

import importlib.util
import json
import os
import unittest
from unittest import mock

import FreeCAD
import FreeCADGui
import FerroCAD
import FerroCADGui
import ferrocad_shell  # noqa: F401  (registers the shell commands)
from FerroCAD import _grid

FEATURE = "grid"
MODS_ROOT = "User parameter:BaseApp/Preferences/Mods"
LEGACY_PREFERENCES = "User parameter:BaseApp/Preferences/View"

MOD_PATH = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "mods",
    "GridOverlay",
    "InitGui.py",
)

#: The canonical order and the display string each default produces.
ATTRIBUTES = {
    "enabled": False,
    "color": "#94a3b8",
    "opacity": 1.0,
    "minor": 10.0,
    "major": 100.0,
    "radius": 0.0,
    "axis_cross": True,
    "shadows": False,
    "shadow_opacity": 0.30,
}


def settings_group(feature, backend):
    return FreeCAD.ParamGet("%s/%s/%s" % (MODS_ROOT, feature, backend))


def provider_prefs():
    return FreeCAD.ParamGet(MODS_ROOT).GetGroup("Providers")


def close_documents():
    for name in list(FreeCAD.listDocuments()):
        FreeCAD.closeDocument(name)


class GridProviderFacadeTests(unittest.TestCase):
    def setUp(self):
        self.grid = FerroCAD.Grid
        self.grid.reset()
        FreeCAD.ParamGet(MODS_ROOT).GetGroup(FEATURE).Clear()
        provider_prefs().Clear()
        FerroCADGui._PROVIDER_APPLIED.clear()
        FerroCADGui._SETTING_REPORTED.clear()
        FerroCADGui._PROVIDER_REPORTED.clear()
        close_documents()

    def tearDown(self):
        for provider in FerroCADGui.providers(FEATURE):
            FerroCADGui.unregisterProvider(FEATURE, provider["backend"])
        FreeCAD.ParamGet(MODS_ROOT).GetGroup(FEATURE).Clear()
        provider_prefs().Clear()
        FerroCADGui._PROVIDER_APPLIED.clear()
        FerroCADGui._SETTING_REPORTED.clear()
        FerroCADGui._PROVIDER_REPORTED.clear()
        self.grid.reset()
        close_documents()

    # -- helpers -------------------------------------------------------------
    def register_native(self):
        FerroCADGui.registerProvider(
            FEATURE,
            "ferrocad",
            "FerroCAD (native)",
            priority=10,
            settings=_grid.SETTINGS_SCHEMA,
        )

    def register_coin(self):
        subset = [
            dict(entry)
            for entry in _grid.SETTINGS_SCHEMA
            if entry["name"] in ("enabled", "color", "opacity", "shadows", "shadow_opacity")
        ]
        FerroCADGui.registerProvider(
            FEATURE, "coin", "Coin (WGSL)", priority=0, settings=subset
        )

    def rows(self):
        return {row["name"]: row for row in FerroCADGui.settings(FEATURE)}

    # -- the schema ----------------------------------------------------------
    def test_the_schema_matches_every_attribute_and_its_default(self):
        self.assertEqual(
            [entry["name"] for entry in _grid.SETTINGS_SCHEMA], list(ATTRIBUTES)
        )
        for entry in _grid.SETTINGS_SCHEMA:
            self.assertEqual(entry["default"], ATTRIBUTES[entry["name"]], entry["name"])
            self.assertIn(entry["kind"], ("bool", "color", "float", "quantity"))

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

        # ``schema()`` is a copy: a caller cannot mutate the canonical list.
        schema = self.grid.schema()
        self.assertEqual([entry["name"] for entry in schema], list(ATTRIBUTES))
        schema[0]["name"] = "mutated"
        self.assertEqual(_grid.SETTINGS_SCHEMA[0]["name"], "enabled")

    # -- one store -----------------------------------------------------------
    def test_the_rows_and_the_attributes_read_the_same_values(self):
        self.register_native()

        self.grid.enabled = True
        self.grid.color = "#FF0000"
        self.grid.opacity = 0.5
        self.grid.minor = 2.5
        self.grid.major = 25.0
        self.grid.radius = 750.0
        self.grid.axis_cross = False
        self.grid.shadows = True
        self.grid.shadow_opacity = 0.4

        values = {name: row["value"] for name, row in self.rows().items()}
        self.assertEqual(
            values,
            {
                "enabled": "True",
                "color": "#ff0000",
                "opacity": "0.5",
                "minor": "2.5 mm",
                "major": "25 mm",
                "radius": "750 mm",
                "axis_cross": "False",
                "shadows": "True",
                "shadow_opacity": "0.4",
            },
        )

        # The reverse direction: a row write is the same parameter the attribute
        # reads, and ``spacing`` stays the minor-step alias.
        self.assertTrue(FerroCADGui.setSetting(FEATURE, "minor", "7 mm")["ok"])
        self.assertTrue(FerroCADGui.setSetting(FEATURE, "color", "#00FF00")["ok"])
        self.assertEqual(self.grid.minor, 7.0)
        self.assertEqual(self.grid.spacing, 7.0)
        self.assertEqual(self.grid.color, "#00ff00")

    def test_set_property_updates_ferrocad_grid(self):
        self.register_native()

        minor = json.loads(
            ferrocad_shell.set_property("Settings/Grid", "minor", "5 mm")
        )
        self.assertTrue(minor["ok"], minor)
        self.assertEqual(self.grid.minor, 5.0)
        self.assertEqual(self.grid.spacing, 5.0)

        enabled = json.loads(
            ferrocad_shell.set_property("Settings/Grid", "enabled", "true")
        )
        self.assertTrue(enabled["ok"], enabled)
        self.assertTrue(self.grid.enabled)

        color = json.loads(
            ferrocad_shell.set_property("Settings/Grid", "color", "#ABCDEF")
        )
        self.assertTrue(color["ok"], color)
        self.assertEqual(self.grid.color, "#abcdef")

        rejected = json.loads(
            ferrocad_shell.set_property("Settings/Grid", "opacity", "2.0")
        )
        self.assertFalse(rejected["ok"])
        self.assertIn("at most 1", rejected["error"])
        self.assertEqual(self.grid.opacity, 1.0, "a rejected row write changes nothing")

    # -- providers -----------------------------------------------------------
    def test_a_provider_switch_changes_the_rows(self):
        self.register_native()
        self.register_coin()

        native = self.rows()
        self.assertIn("minor", native)
        self.assertIn("radius", native)
        self.assertIn("axis_cross", native)

        switched = json.loads(
            ferrocad_shell.set_property("Settings/Grid", "provider", "Coin (WGSL)")
        )
        self.assertTrue(switched["ok"], switched)
        self.assertEqual(FerroCADGui.activeProvider(FEATURE), "coin")

        coin = self.rows()
        self.assertEqual(
            list(coin), ["enabled", "color", "opacity", "shadows", "shadow_opacity"]
        )
        self.assertNotIn("minor", coin)
        self.assertNotIn("axis_cross", coin)

        back = json.loads(
            ferrocad_shell.set_property("Settings/Grid", "provider", "ferrocad")
        )
        self.assertTrue(back["ok"], back)
        self.assertIn("minor", self.rows())

    def test_grid_summary_names_the_active_provider_and_spacing(self):
        self.assertEqual(
            json.loads(ferrocad_shell.grid_summary()),
            {
                "feature": "grid",
                "backend": None,
                "label": None,
                "spacing": None,
                "providers": [],
            },
            "the empty state keeps the same shape",
        )

        self.register_native()
        self.grid.spacing = 2.0
        summary = json.loads(ferrocad_shell.grid_summary())
        self.assertEqual(summary["feature"], "grid")
        self.assertEqual(summary["backend"], "ferrocad")
        self.assertEqual(summary["label"], "FerroCAD (native)")
        self.assertEqual(summary["spacing"], "2 mm")
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
        FerroCADGui.setActiveProvider(FEATURE, "coin")
        summary = json.loads(ferrocad_shell.grid_summary())
        self.assertEqual(summary["backend"], "coin")
        self.assertEqual(summary["label"], "Coin (WGSL)")
        self.assertEqual(
            summary["spacing"],
            "2 mm",
            "a backend without a spacing row reports the one grid's step",
        )
        self.assertEqual(
            [provider["backend"] for provider in summary["providers"]],
            ["ferrocad", "coin"],
        )
        self.assertEqual(
            [provider["active"] for provider in summary["providers"]], [False, True]
        )

    # -- the commands are aliases of the Settings node -----------------------
    def test_view_grid_settings_selects_the_settings_node_when_a_provider_exists(self):
        dialog = FreeCADGui.runCommand("View_GridSettings")
        self.assertIn("fields", dialog, "the modal stays as the headless fallback")

        self.register_native()
        self.assertEqual(
            FreeCADGui.runCommand("View_GridSettings"), {"select": "Settings/Grid"}
        )

    def test_the_native_mod_advertises_the_canonical_schema(self):
        # The command registration is another suite's concern; swallow it so this
        # test leaves the global command registry exactly as it found it.
        spec = importlib.util.spec_from_file_location(
            "GridOverlay_InitGui_s4_test", MOD_PATH
        )
        module = importlib.util.module_from_spec(spec)
        self.addCleanup(FerroCADGui.unregisterProvider, FEATURE, "ferrocad")
        with mock.patch.object(FreeCADGui, "addCommand"):
            spec.loader.exec_module(module)

        self.assertEqual(FerroCADGui.activeProvider(FEATURE), "ferrocad")
        self.assertEqual(
            [row["name"] for row in FerroCADGui.settings(FEATURE)],
            list(ATTRIBUTES),
        )
        self.assertEqual(module.settings_node(), "Settings/Grid")


if __name__ == "__main__":
    unittest.main()
