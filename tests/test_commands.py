"""Tests for the ``FreeCADGui`` command registry (A0).

See ``docs/developer/workbenches/actions-and-tools.md``. These run against the real facade and need the
``ferrocad`` extension on ``PYTHONPATH`` (the headless CI job builds it first).
"""

import importlib.util
import json
import pathlib
import unittest
from unittest import mock

import FreeCAD
import FreeCADGui
import FerroCADGui
import ferrocad_shell  # noqa: F401  (registers the host's commands)

#: The mods that own the backend settings dialogs, and their command classes.
BACKEND_SETTINGS_COMMANDS = {
    "GridOverlay_Settings": ("GridOverlay", "GridOverlayCommand"),
    "GridOverlayCoin_Settings": ("GridOverlayCoin", "GridOverlayCoinCommand"),
    "AxisOverlay_Settings": ("AxisOverlay", "AxisOverlayCommand"),
    "AxisOverlayCoin_Settings": ("AxisOverlayCoin", "AxisOverlayCoinCommand"),
}
MODS_ROOT = pathlib.Path(__file__).resolve().parents[1] / "mods"


def load_mod_command(directory, class_name):
    """Instantiate one mod's command without its import-time side effects.

    Importing ``InitGui.py`` normally registers the command and the provider;
    neither is needed to read ``GetResources()``, and leaving them registered
    would leak into later suites. The overlay manager is not built because the
    startup hook is gated on an active document.
    """
    path = MODS_ROOT / directory / "InitGui.py"
    spec = importlib.util.spec_from_file_location("dev_only_%s" % directory, path)
    module = importlib.util.module_from_spec(spec)
    with mock.patch.object(FreeCADGui, "addCommand"), mock.patch.object(
        FreeCADGui, "removeCommand"
    ), mock.patch.object(
        FerroCADGui, "registerProvider", return_value=False
    ), mock.patch.object(FreeCAD, "activeDocument", return_value=None):
        spec.loader.exec_module(module)
    return getattr(module, class_name)()


class _Command:
    def __init__(self, status="ran"):
        self.status = status

    def GetResources(self):
        return {
            "MenuText": "Test Action",
            "ToolTip": "a test action",
            "Group": "Test",
            "CmdType": "ForEdit",
        }

    def IsActive(self):
        return True

    def Activated(self):
        return {"status": self.status}


class _Minimal:
    def Activated(self):
        return None


class _RaisingActivated:
    def Activated(self):
        raise RuntimeError("boom")


class _RaisingIsActive:
    def IsActive(self):
        raise RuntimeError("inactive")

    def Activated(self):
        return None


class _DevOnly:
    def GetResources(self):
        return {
            "MenuText": "Dev Only",
            "FerroCAD_DevOnly": "true",
        }

    def Activated(self):
        return None


class _RaisingResources:
    def GetResources(self):
        raise RuntimeError("no resources")

    def Activated(self):
        return None


class CommandRegistryTests(unittest.TestCase):
    def setUp(self):
        self.names = []

    def tearDown(self):
        for name in self.names:
            FreeCADGui.removeCommand(name)

    def add(self, name, command):
        FreeCADGui.addCommand(name, command)
        self.names.append(name)

    def entry(self, name):
        return next(c for c in FreeCADGui._command_list() if c["id"] == name)

    def replace(self, name, command):
        """Register ``command`` as ``name`` and restore what was there before."""
        previous = FreeCADGui._COMMANDS.pop(name, None)
        FreeCADGui.addCommand(name, command)

        def restore():
            FreeCADGui.removeCommand(name)
            if previous is not None:
                FreeCADGui.addCommand(name, previous)

        self.addCleanup(restore)

    def test_register_list_run(self):
        self.add("Test_Run", _Command("done"))
        self.assertIn("Test_Run", FreeCADGui.listCommands())
        self.assertEqual(FreeCADGui.runCommand("Test_Run"), {"status": "done"})

        entry = self.entry("Test_Run")
        self.assertEqual(entry["menu"], "Test Action")
        self.assertEqual(entry["tooltip"], "a test action")
        self.assertEqual(entry["group"], "Test")
        self.assertEqual(entry["cmd_type"], "ForEdit")
        self.assertTrue(entry["enabled"])
        self.assertFalse(entry["checked"])

    def test_resource_defaults(self):
        self.add("Test_Min", _Minimal())
        entry = self.entry("Test_Min")
        self.assertEqual(entry["menu"], "Test_Min")
        self.assertEqual(entry["tooltip"], "")
        self.assertTrue(entry["enabled"], "IsActive defaults to true")
        self.assertFalse(entry["dev_only"], "a missing GetResources is not dev-only")

    def test_the_command_list_forwards_dev_only(self):
        self.add("Test_DevOnly", _DevOnly())
        self.add("Test_Header", _Minimal())
        self.assertTrue(self.entry("Test_DevOnly")["dev_only"])
        self.assertFalse(self.entry("Test_Header")["dev_only"])

    def test_a_raising_get_resources_is_not_dev_only(self):
        self.add("Test_NoResources", _RaisingResources())
        with mock.patch.object(FreeCADGui, "_report_once"):
            self.assertFalse(self.entry("Test_NoResources")["dev_only"])

    def test_the_backend_settings_commands_are_dev_only(self):
        for command_id, (directory, class_name) in BACKEND_SETTINGS_COMMANDS.items():
            with self.subTest(command=command_id):
                command = load_mod_command(directory, class_name)
                self.assertEqual(
                    command.GetResources()["FerroCAD_DevOnly"],
                    "true",
                    "%s stayed in the header popover" % command_id,
                )
                self.replace(command_id, command)
                self.assertTrue(self.entry(command_id)["dev_only"])

    def test_the_host_commands_split_dev_only_from_the_header(self):
        entries = {
            entry["id"]: entry for entry in json.loads(ferrocad_shell.commands())
        }
        self.assertTrue(entries["Mods_Settings"]["dev_only"])
        self.assertFalse(entries["App_ToggleAxisCross"]["dev_only"])
        self.assertFalse(entries["View_AxisSettings"]["dev_only"])

    def test_object_verbs_live_on_the_radial_surface(self):
        entries = {
            entry["id"]: entry for entry in json.loads(ferrocad_shell.commands())
        }
        self.assertEqual(entries["App_Delete"]["surface"], "radial")
        self.assertEqual(entries["App_Delete"]["tone"], "destructive")
        self.assertEqual(entries["App_Move"]["surface"], "radial")
        self.assertEqual(entries["App_Move"]["tone"], "structural")
        # The bottom strip is creation + search: object verbs are not dock rows.
        self.assertNotEqual(entries["App_Delete"]["surface"], "dock")
        self.assertNotEqual(entries["App_Move"]["surface"], "dock")

    def test_a_duplicate_id_raises(self):
        self.add("Test_Dup", _Command())
        with self.assertRaises(ValueError):
            FreeCADGui.addCommand("Test_Dup", _Command())

    def test_a_command_without_activated_raises(self):
        with self.assertRaises(TypeError):
            FreeCADGui.addCommand("Test_NoAct", object())
        self.assertNotIn("Test_NoAct", FreeCADGui.listCommands())

    def test_a_raising_activated_is_isolated(self):
        self.add("Test_Broken", _RaisingActivated())
        result = FreeCADGui._run("Test_Broken")
        self.assertFalse(result["ok"])
        self.assertIn("RuntimeError: boom", result["error"])

    def test_a_raising_is_active_disables_only_that_command(self):
        self.add("Test_BrokenActive", _RaisingIsActive())
        self.add("Test_Healthy", _Minimal())
        self.assertFalse(self.entry("Test_BrokenActive")["enabled"])
        self.assertTrue(self.entry("Test_Healthy")["enabled"])
        # `_run` does not consult IsActive, so the command can still run.
        self.assertTrue(FreeCADGui._run("Test_BrokenActive")["ok"])

    def test_an_unknown_command(self):
        self.assertNotIn("Test_None", FreeCADGui.listCommands())
        result = FreeCADGui._run("Test_None")
        self.assertFalse(result["ok"])
        self.assertIn("Test_None", result["error"])
        with self.assertRaises(KeyError):
            FreeCADGui.runCommand("Test_None")


class CatalogTests(unittest.TestCase):
    """The built-in metadata comes from one table (D10).

    ``python/ferrocad_shell/catalog.json`` is generated from
    ``crates/ferrocad_shell_api/src/catalog.rs`` by
    ``cargo run -p ferrocad_shell_api --example catalog``; that crate's test fails
    when the file is stale, and these fail when this registry and the file
    disagree.
    """

    @classmethod
    def setUpClass(cls):
        path = pathlib.Path(ferrocad_shell.__file__).with_name("catalog.json")
        cls.catalog = json.loads(path.read_text(encoding="utf-8"))
        cls.commands = {entry["id"]: entry for entry in cls.catalog["commands"]}

    def test_python_and_the_catalogue_have_the_same_builtins(self):
        self.assertEqual(
            set(ferrocad_shell._BUILTIN_COMMANDS),
            set(self.commands),
            "a built-in is registered on one side only: add it to "
            "crates/ferrocad_shell_api/src/catalog.rs and regenerate catalog.json",
        )

    def test_every_catalogued_command_is_registered(self):
        live = set(FreeCADGui.listCommands())
        for command_id in self.commands:
            with self.subTest(command=command_id):
                self.assertIn(command_id, live)

    def test_the_registry_row_matches_the_catalogue(self):
        rows = {
            entry["id"]: entry for entry in json.loads(ferrocad_shell.commands())
        }
        for command_id, entry in self.commands.items():
            with self.subTest(command=command_id):
                row = rows[command_id]
                self.assertEqual(row["menu"], entry["menu"])
                self.assertEqual(row["tooltip"], entry["tooltip"])
                self.assertEqual(row["accel"], entry.get("accel", ""))
                self.assertEqual(row["group"], entry["group"])
                self.assertEqual(row["cmd_type"], entry["cmd_type"])
                self.assertEqual(row["surface"], entry.get("surface", ""))
                self.assertEqual(row["short"], entry.get("short", ""))
                self.assertEqual(row["tone"], entry.get("tone", ""))
                self.assertEqual(row["parent"], entry.get("parent", ""))
                self.assertEqual(row["wheel"], entry.get("wheel", False))
                self.assertEqual(row["checkable"], entry.get("checkable", False))
                self.assertEqual(row["dev_only"], entry.get("dev_only", False))

    def test_the_move_form_is_the_catalogues(self):
        spec = ferrocad_shell._MoveCommand().Activated()
        dialog = self.catalog["dialogs"]["App_Move"]
        self.assertEqual(spec["title"], dialog["title"])
        self.assertEqual(spec["accept"], dialog["accept"])
        self.assertEqual(spec["presentation"], dialog["presentation"])
        self.assertEqual(spec["preview"], dialog["preview"])
        self.assertEqual(spec["gizmo"], dialog["gizmo"])
        self.assertEqual(spec["fields"], dialog["fields"])


class ProviderFeaturesTests(unittest.TestCase):
    """The Dev Tools providers panel's data."""

    def test_provider_features_groups_by_feature(self):
        feature = "provider_features_test"
        FerroCADGui.registerProvider(feature, "a", "A", priority=1)
        self.addCleanup(FerroCADGui.unregisterProvider, feature, "a")

        features = {
            entry["feature"]: entry
            for entry in json.loads(ferrocad_shell.provider_features())
        }
        self.assertIn(feature, features)
        self.assertEqual(
            features[feature]["providers"],
            [{"backend": "a", "label": "A", "active": True, "command": None}],
        )


if __name__ == "__main__":
    unittest.main()
