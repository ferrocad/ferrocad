"""Tests for the GridOverlay mod (G6, the A2 shipping form).

The mod is a thin client of the semantic grid API: ``Activated`` returns the
shell's dialog spec, ``on_accept`` writes ``FerroCAD.Grid`` and returns its
``{"view": …}`` request, and a build without ``FerroCAD.Grid`` reports cleanly
instead of falling back to the upstream Coin/GLSL path. See
``docs/workbenches/grid-overlay-plan.md`` §3.2–§3.3, §5 G6.

``InitGui.py`` is loaded **by path** with ``importlib`` so the test does not
depend on ``sys.path`` order (or on the shell's workbench loader).
"""

import importlib.util
import pathlib
import sys
import types
import unittest
from unittest import mock

import FreeCADGui
import FerroCAD
import FerroCADGui

MOD_PATH = (
    pathlib.Path(__file__).resolve().parents[1]
    / "mods"
    / "GridOverlay"
    / "InitGui.py"
)
COMMAND = "GridOverlay_Settings"

#: The keys the shell's ``ViewRequest`` understands (``docs/workbenches/actions-and-tools.md`` §7).
VIEW_KEYS = {
    "grid",
    "axis_cross",
    "grid_minor",
    "grid_major",
    "grid_radius",
    "grid_color",
    "grid_alpha",
}

KNOBS = {"enabled", "color", "opacity", "spacing", "shadows", "shadow_opacity"}

_LOADED = None


def load_mod():
    """Load the mod once; return ``(module, module names it imported)``.

    Any previous registration is dropped first so a re-run in one interpreter
    does not trip the registry's duplicate-id check; the fact that registration
    happens at load time is what the tests then observe.
    """
    global _LOADED
    if _LOADED is not None:
        return _LOADED
    before = set(sys.modules)
    FreeCADGui.removeCommand(COMMAND)
    spec = importlib.util.spec_from_file_location("GridOverlay_InitGui", MOD_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    _LOADED = (module, set(sys.modules) - before)
    return _LOADED


class GridOverlayForkTests(unittest.TestCase):
    def setUp(self):
        self.grid = FerroCAD.Grid
        self.grid.reset()
        for entry in FerroCADGui.providers():
            FerroCADGui.unregisterProvider(entry["feature"], entry["backend"])
        self.mod, self.imported = load_mod()
        # This suite exercises the FerroCAD-native backend as the active provider, even
        # when the Coin suite ran first or another suite cleared the registry
        # (``load_mod`` caches the module, so re-register explicitly).
        if "ferrocad" not in [p["backend"] for p in FerroCADGui.providers("grid")]:
            self.mod.register_provider()
        FerroCADGui.setActiveProvider("grid", "ferrocad")

    def tearDown(self):
        FerroCADGui.unregisterProvider("grid", "ferrocad")
        FreeCADGui._cancel_all()
        self.grid.reset()

    def spec(self):
        return FreeCADGui.runCommand(COMMAND)

    def test_the_command_is_registered_with_the_upstream_resources(self):
        self.assertIn(COMMAND, FreeCADGui.listCommands())
        entry = next(
            item
            for item in FreeCADGui._command_list()
            if item["id"] == COMMAND
        )
        self.assertEqual(entry["menu"], "Grid Overlay")
        self.assertTrue(entry["tooltip"])
        self.assertTrue(entry["enabled"], "IsActive() returns True")

        resources = self.mod.GridOverlayCommand().GetResources()
        self.assertTrue(resources["Pixmap"].endswith("grid.svg"))

    def test_activated_returns_a_dialog_spec_with_the_six_knobs(self):
        spec = self.spec()
        self.assertIn("fields", spec)
        self.assertTrue(callable(spec["on_accept"]))
        self.assertEqual(spec["title"], "Grid Overlay Settings")
        self.assertEqual(spec["accept"], "Apply")

        fields = {field["name"]: field for field in spec["fields"]}
        self.assertEqual(set(fields), KNOBS)
        kinds = {name: field["kind"] for name, field in fields.items()}
        self.assertEqual(
            kinds,
            {
                "enabled": "bool",
                "color": "text",
                "opacity": "float",
                "spacing": "quantity",
                "shadows": "bool",
                "shadow_opacity": "float",
            },
        )
        # The seeds are the current FerroCAD.Grid state (defaults here).
        snapshot = self.grid.snapshot()
        self.assertEqual(fields["enabled"]["value"], snapshot["grid"])
        self.assertEqual(fields["color"]["value"], snapshot["grid_color"])
        self.assertEqual(fields["opacity"]["value"], snapshot["grid_alpha"])
        self.assertEqual(fields["spacing"]["value"], snapshot["grid_minor"])
        self.assertEqual(fields["shadows"]["value"], self.grid.shadows)
        self.assertEqual(
            fields["shadow_opacity"]["value"], self.grid.shadow_opacity
        )

    def test_the_spec_is_seeded_from_the_live_settings(self):
        self.grid.enabled = True
        self.grid.color = "#123456"
        self.grid.opacity = 0.5
        self.grid.minor = 2.0
        self.grid.major = 20.0
        self.grid.shadows = True
        self.grid.shadow_opacity = 0.25

        fields = {field["name"]: field["value"] for field in self.spec()["fields"]}
        self.assertEqual(
            fields,
            {
                "enabled": True,
                "color": "#123456",
                "opacity": 0.5,
                "spacing": 2.0,
                "shadows": True,
                "shadow_opacity": 0.25,
            },
        )

    def test_on_accept_writes_the_grid_and_returns_the_view_request(self):
        result = self.spec()["on_accept"](
            {
                "enabled": True,
                "color": "#f87171",
                "opacity": 0.5,
                "spacing": 2.0,
                "shadows": True,
                "shadow_opacity": 0.25,
            }
        )

        self.assertIn("view", result)
        self.assertEqual(set(result["view"]), VIEW_KEYS)

        snapshot = self.grid.snapshot()
        self.assertTrue(snapshot["grid"])
        self.assertEqual(snapshot["grid_color"], "#f87171")
        self.assertEqual(snapshot["grid_alpha"], 0.5)
        self.assertEqual(snapshot["grid_minor"], 2.0)
        self.assertEqual(snapshot["grid_major"], 20.0)
        self.assertTrue(self.grid.shadows)
        self.assertEqual(self.grid.shadow_opacity, 0.25)

        self.assertEqual(
            result["view"],
            {
                "grid": True,
                "axis_cross": True,
                "grid_minor": 2.0,
                "grid_major": 20.0,
                "grid_radius": 0.0,
                "grid_color": "#f87171",
                "grid_alpha": 0.5,
            },
        )

    def test_the_shell_dialog_path_applies_the_view_request(self):
        run = FreeCADGui._run(COMMAND)
        self.assertTrue(run["ok"], run)
        self.assertIn("token", run)
        self.assertTrue(run["dialog"]["title"])
        self.assertEqual(
            {field["name"] for field in run["dialog"]["fields"]}, KNOBS
        )

        accepted = FreeCADGui._accept(
            run["token"], {"enabled": True, "color": "#00ff00"}
        )
        self.assertTrue(accepted["ok"], accepted)
        self.assertEqual(set(accepted["view"]), VIEW_KEYS)
        self.assertTrue(accepted["view"]["grid"])
        self.assertEqual(accepted["view"]["grid_color"], "#00ff00")

    def test_a_partial_accept_keeps_the_other_values(self):
        self.grid.color = "#00ff00"
        result = self.spec()["on_accept"]({"opacity": 0.4})
        self.assertEqual(result["view"]["grid_color"], "#00ff00")
        self.assertEqual(result["view"]["grid_alpha"], 0.4)

    def test_a_bad_value_raises_and_leaves_the_store_alone(self):
        with self.assertRaises(ValueError):
            self.spec()["on_accept"]({"color": "not a colour"})
        self.assertEqual(self.grid.color, "#94a3b8")

    def test_reset_restores_the_defaults(self):
        self.spec()["on_accept"](
            {
                "enabled": True,
                "color": "#000000",
                "opacity": 0.2,
                "spacing": 3.0,
                "shadows": True,
                "shadow_opacity": 0.1,
            }
        )
        self.grid.reset()

        snapshot = self.grid.snapshot()
        self.assertEqual(
            snapshot,
            {
                "grid": False,
                "axis_cross": True,
                "grid_minor": 10.0,
                "grid_major": 100.0,
                "grid_radius": None,
                "grid_color": "#94a3b8",
                "grid_alpha": 1.0,
                "shadows": False,
                "shadow_opacity": 0.30,
            },
        )
        self.assertFalse(self.grid.shadows)
        self.assertAlmostEqual(self.grid.shadow_opacity, 0.30)
        fields = {field["name"]: field["value"] for field in self.spec()["fields"]}
        self.assertEqual(fields["color"], "#94a3b8")
        self.assertEqual(fields["spacing"], 10.0)

    def test_loading_the_mod_does_not_pull_qt_or_coin(self):
        for name in self.imported:
            self.assertNotIn(
                name.split(".")[0],
                {"PySide", "PySide2", "PySide6", "pivy", "coin"},
                "loading the mod imported %s" % name,
            )
        for attribute in ("coin", "PySide", "pivy", "SoSeparator", "getSceneGraph"):
            self.assertFalse(hasattr(self.mod, attribute), attribute)

    def test_the_discriminator_reads_getattr_of_the_module(self):
        self.assertIs(self.mod.grid_api(), self.grid)
        self.assertIsNone(self.mod.grid_api(types.SimpleNamespace()))

    def test_a_build_without_grid_reports_instead_of_falling_back(self):
        with mock.patch.object(self.mod, "grid_api", return_value=None):
            result = self.mod.GridOverlayCommand().Activated()
            accepted = self.mod.apply_settings({"enabled": True})

        self.assertNotIn("fields", result)
        self.assertIn("status", result)
        self.assertIn("FerroCAD", result["status"])
        self.assertIn("FerroCAD.Grid", result["status"])
        self.assertNotIn("view", result)
        self.assertIn("status", accepted)
        self.assertNotIn("view", accepted)

    def test_the_real_facade_without_grid_is_detected(self):
        # A build without the FerroCAD namespace has no `Grid` attribute, so
        # `getattr` raises and the helper must return None rather than
        # propagating AttributeError.
        original = FerroCAD.__getattr__

        def upstream_freecad(name):
            if name == "Grid":
                raise AttributeError(
                    "module 'FerroCAD' has no attribute 'Grid'"
                )
            return original(name)

        with mock.patch.object(FerroCAD, "__getattr__", upstream_freecad):
            self.assertIsNone(self.mod.grid_api())
            self.assertIn("status", self.mod.GridOverlayCommand().Activated())


if __name__ == "__main__":
    unittest.main()
