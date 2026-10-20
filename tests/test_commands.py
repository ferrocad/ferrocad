"""Tests for the ``FreeCADGui`` command registry (A0).

See ``docs/actions-and-tools.md``. These run against the real facade and need the
``ferrocad`` extension on ``PYTHONPATH`` (the headless CI job builds it first).
"""

import unittest

import FreeCADGui


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


if __name__ == "__main__":
    unittest.main()
