"""Tests for the file-backed parameter store (``docs/tasks/settings-model.md`` S3).

``FreeCAD.ParamGet`` stays in memory while no file is configured. These tests
point it at a temporary file through the FerroCAD-only
``FerroCAD.set_parameter_file`` and verify the whole contract: typed round-trip,
groups, a simulated restart, corrupt input, unknown keys, the persisted provider
choice (including a stale backend), and that ``Export``/``Import`` are unchanged.

Every test gets a fresh in-memory store and a fresh temp directory; ``tearDown``
restores the previous path and the previous store.
"""

import contextlib
import io
import json
import os
import shutil
import tempfile
import unittest

import FreeCAD
import FerroCAD
import FerroCADGui

PREFERENCES = "User parameter:BaseApp/Preferences/PersistSettings"
ROOT = "User parameter"
GROUP = "BaseApp/Preferences/PersistSettings"
FEATURE = "persist_grid"


class ParameterPersistenceTests(unittest.TestCase):
    def setUp(self):
        self._tempdir = tempfile.mkdtemp(prefix="ferrocad-params-")
        self.path = os.path.join(self._tempdir, "nested", "parameters.json")
        self._previous_file = FerroCAD.parameter_file()
        self._previous_stores = FreeCAD._parameter_stores
        # A private store per test: a simulated restart is just a fresh dict.
        FreeCAD._parameter_stores = {}
        self._features = set()
        FerroCAD.set_parameter_file(self.path)

    def tearDown(self):
        for feature in self._features:
            for provider in FerroCADGui.providers(feature):
                FerroCADGui.unregisterProvider(feature, provider["backend"])
            FerroCADGui._PROVIDER_APPLIED.pop(feature, None)

        FerroCAD.set_parameter_file(self._previous_file)
        FreeCAD._parameter_stores = self._previous_stores
        shutil.rmtree(self._tempdir, ignore_errors=True)

    # -- helpers -------------------------------------------------------------
    def restart(self):
        """Drop the in-memory store and re-arm the load from the same file."""
        FreeCAD._parameter_stores = {}
        FerroCAD.set_parameter_file(self.path)

    def read_file(self):
        with open(self.path, encoding="utf-8") as handle:
            return json.load(handle)

    def register(self, backend, label, priority=0):
        self._features.add(FEATURE)
        return FerroCADGui.registerProvider(
            FEATURE, backend, label, priority=priority
        )

    # -- the file ------------------------------------------------------------
    def test_the_controls_are_ferrocad_only(self):
        self.assertIn("set_parameter_file", FerroCAD.__all__)
        self.assertIn("parameter_file", FerroCAD.__all__)
        self.assertTrue(callable(FerroCAD.set_parameter_file))
        self.assertTrue(callable(FerroCAD.parameter_file))
        self.assertFalse(hasattr(FreeCAD, "set_parameter_file"))
        self.assertFalse(hasattr(FreeCAD, "parameter_file"))

    def test_an_explicit_path_is_used_even_when_headless(self):
        self.assertFalse(FreeCAD.GuiUp)
        self.assertEqual(FerroCAD.parameter_file(), self.path)
        # A read never creates the file; only a mutation does.
        FreeCAD.ParamGet(PREFERENCES).GetInt("Missing", 0)
        self.assertFalse(os.path.exists(self.path))

    def test_typed_values_and_groups_round_trip(self):
        prefs = FreeCAD.ParamGet(PREFERENCES)
        prefs.GetGroup("Empty")  # a group with no values is still a group
        prefs.SetInt("Count", 7)
        prefs.SetUnsigned("Mask", 0xFFFFFFFF)
        prefs.SetBool("Enabled", True)
        prefs.SetFloat("Scale", 1.25)
        prefs.SetString("Label", "hello")
        prefs.GetGroup("Child").SetInt("Depth", 2)

        data = self.read_file()
        self.assertEqual(data["version"], 1)
        self.assertEqual(len(data["values"]), 6)
        self.assertIn({"root": ROOT, "path": GROUP + "/Empty"}, data["groups"])

        self.restart()

        prefs = FreeCAD.ParamGet(PREFERENCES)
        self.assertEqual(prefs.GetInt("Count"), 7)
        self.assertEqual(prefs.GetInts(), ["Count", "Mask"])
        self.assertEqual(prefs.GetUnsigned("Mask"), 0xFFFFFFFF)
        self.assertTrue(prefs.GetBool("Enabled"))
        self.assertEqual(prefs.GetFloat("Scale"), 1.25)
        self.assertEqual(prefs.GetString("Label"), "hello")
        self.assertEqual(prefs.GetGroups(), ["Child", "Empty"])
        self.assertTrue(prefs.HasGroup("Empty"))
        self.assertEqual(prefs.GetGroup("Child").GetInt("Depth"), 2)

    def test_a_corrupt_file_starts_empty_and_reports_once(self):
        os.makedirs(os.path.dirname(self.path), exist_ok=True)
        with open(self.path, "w", encoding="utf-8") as handle:
            handle.write("{not json")

        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            self.assertEqual(FreeCAD.ParamGet(PREFERENCES).GetInt("Count", 3), 3)
            # Reloading the same path must not report the same problem again.
            self.restart()
            self.assertEqual(FreeCAD.ParamGet(PREFERENCES).GetInt("Count", 3), 3)

        messages = [
            line
            for line in stderr.getvalue().splitlines()
            if "parameter file" in line
        ]
        self.assertEqual(len(messages), 1, stderr.getvalue())

        # Reading is side-effect free: the broken file is only replaced by a write.
        with open(self.path, encoding="utf-8") as handle:
            self.assertEqual(handle.read(), "{not json")

        FreeCAD.ParamGet(PREFERENCES).SetInt("Count", 9)
        data = self.read_file()
        self.assertEqual(data["version"], 1)
        self.assertIn(
            {
                "root": ROOT,
                "group": GROUP,
                "name": "Count",
                "kind": "int",
                "value": 9,
            },
            data["values"],
        )
        # The atomic write leaves no temp file behind.
        self.assertEqual(os.listdir(os.path.dirname(self.path)), ["parameters.json"])

    def test_unknown_keys_are_ignored_with_one_report(self):
        os.makedirs(os.path.dirname(self.path), exist_ok=True)
        payload = {
            "version": 1,
            "groups": [],
            "values": [
                {
                    "root": ROOT,
                    "group": GROUP,
                    "name": "Loaded",
                    "kind": "int",
                    "value": 42,
                }
            ],
            "from_the_future": {"ignored": True},
        }
        with open(self.path, "w", encoding="utf-8") as handle:
            json.dump(payload, handle)

        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            self.assertEqual(FreeCAD.ParamGet(PREFERENCES).GetInt("Loaded"), 42)
            self.restart()
            self.assertEqual(FreeCAD.ParamGet(PREFERENCES).GetInt("Loaded"), 42)

        messages = [
            line
            for line in stderr.getvalue().splitlines()
            if "unknown key" in line
        ]
        self.assertEqual(len(messages), 1, stderr.getvalue())
        self.assertIn("from_the_future", messages[0])

    def test_the_default_file_is_only_enabled_in_a_gui_session(self):
        previous_gui = FreeCAD.GuiUp
        previous_xdg = os.environ.get("XDG_CONFIG_HOME")
        previous_env = os.environ.get("FERROCAD_PARAM_FILE")
        try:
            os.environ.pop("FERROCAD_PARAM_FILE", None)
            os.environ["XDG_CONFIG_HOME"] = self._tempdir
            FerroCAD.set_parameter_file(None)

            FreeCAD.GuiUp = 0
            self.assertIsNone(FerroCAD.parameter_file())

            FreeCAD.GuiUp = 1
            self.assertEqual(
                FerroCAD.parameter_file(),
                os.path.join(self._tempdir, "ferrocad", "parameters.json"),
            )
        finally:
            FreeCAD.GuiUp = previous_gui
            if previous_xdg is None:
                os.environ.pop("XDG_CONFIG_HOME", None)
            else:
                os.environ["XDG_CONFIG_HOME"] = previous_xdg
            if previous_env is None:
                os.environ.pop("FERROCAD_PARAM_FILE", None)
            else:
                os.environ["FERROCAD_PARAM_FILE"] = previous_env
            FerroCAD.set_parameter_file(self.path)

    def test_the_environment_variable_wins_over_the_default(self):
        previous_gui = FreeCAD.GuiUp
        previous_xdg = os.environ.get("XDG_CONFIG_HOME")
        previous_env = os.environ.get("FERROCAD_PARAM_FILE")
        try:
            os.environ["XDG_CONFIG_HOME"] = self._tempdir
            os.environ["FERROCAD_PARAM_FILE"] = self.path
            FerroCAD.set_parameter_file(None)
            FreeCAD.GuiUp = 1
            self.assertEqual(FerroCAD.parameter_file(), self.path)
        finally:
            FreeCAD.GuiUp = previous_gui
            if previous_xdg is None:
                os.environ.pop("XDG_CONFIG_HOME", None)
            else:
                os.environ["XDG_CONFIG_HOME"] = previous_xdg
            if previous_env is None:
                os.environ.pop("FERROCAD_PARAM_FILE", None)
            else:
                os.environ["FERROCAD_PARAM_FILE"] = previous_env
            FerroCAD.set_parameter_file(self.path)

    # -- the provider choice -------------------------------------------------
    def test_the_active_provider_survives_a_restart(self):
        self.register("ferrocad", "Native", priority=1)
        self.register("coin", "Coin (WGSL)", priority=0)
        self.assertEqual(FerroCADGui.activeProvider(FEATURE), "ferrocad")

        FerroCADGui.setActiveProvider(FEATURE, "coin")
        self.restart()

        self.assertEqual(FerroCADGui.activeProvider(FEATURE), "coin")
        persisted = FreeCAD.ParamGet(FerroCADGui.PROVIDER_PREFERENCES).GetGroup(
            "Providers"
        )
        self.assertEqual(persisted.GetString(FEATURE), "coin")

    def test_an_unknown_saved_backend_falls_back_to_the_default(self):
        self.register("ferrocad", "Native", priority=1)
        self.register("coin", "Coin (WGSL)", priority=0)

        # A backend that was removed (or renamed) since the last session.
        FreeCAD.ParamGet(FerroCADGui.PROVIDER_PREFERENCES).GetGroup(
            "Providers"
        ).SetString(FEATURE, "ghost")
        self.restart()

        self.assertEqual(FerroCADGui.activeProvider(FEATURE), "ferrocad")

    # -- import/export -------------------------------------------------------
    def test_export_and_import_are_unchanged_and_persist(self):
        prefs = FreeCAD.ParamGet(PREFERENCES)
        prefs.SetInt("Count", 4)
        prefs.GetGroup("Deep").SetString("Name", "x")

        exported = os.path.join(self._tempdir, "exported.json")
        prefs.Export(exported)
        with open(exported, encoding="utf-8") as handle:
            data = json.load(handle)
        self.assertEqual(data["\tCount\tint"], 4)
        self.assertEqual(data["Deep\tName\tstring"], "x")

        target = FreeCAD.ParamGet(PREFERENCES + "Imported")
        target.Import(exported)
        self.assertEqual(target.GetInt("Count"), 4)
        self.assertEqual(target.GetGroup("Deep").GetString("Name"), "x")

        self.restart()

        restored = FreeCAD.ParamGet(PREFERENCES + "Imported")
        self.assertEqual(restored.GetInt("Count"), 4)
        self.assertEqual(restored.GetGroup("Deep").GetString("Name"), "x")


if __name__ == "__main__":
    unittest.main()
