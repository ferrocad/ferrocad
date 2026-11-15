"""The workbench host: registration, the declaration tables, and the lifecycle.

A workbench is a **declaration**, not a window. ``Gui.Workbench`` is the base upstream
workbenches subclass; ``addWorkbench`` registers it, ``activateWorkbench`` initialises it
once (that is where it registers its commands and appends its toolbars and menus), and the
shell reads the result through ``ferrocad_shell.workbenches`` /
``ferrocad_shell.activate_workbench``.

The Qt half — a workbench's ``Activated``, which puts a dock on a main window — runs only
where a host has adopted a real Qt window (``FreeCADGui._set_gui_host``). The headless
shell never does, so activation stops after the declarations; that split is what lets
``../simplecad``'s ``InitGui.py`` run here at all.

Run::

    python3 tools/run_tests.py --extension base
"""

from __future__ import annotations

import json
import os
import sys
import unittest

import FreeCADGui as Gui
import ferrocad_shell

#: Where the sibling SimpleCAD checkout would be, when the suite is run from the repo.
SIMPLECAD = os.path.abspath(
    os.environ.get("SIMPLECAD_DIR")
    or os.environ.get("FERROCAD_SIMPLECAD")
    or os.path.join(os.path.dirname(__file__), "..", "..", "simplecad")
)


class _Command:
    """The minimum command shape: one callable."""

    def __init__(self):
        self.ran = 0

    def GetResources(self):
        return {"MenuText": "Bread", "ToolTip": "Slice it"}

    def Activated(self):
        self.ran += 1


class _Group:
    """A toolbar dropdown: it lists ids and runs nothing itself."""

    def GetResources(self):
        return {"MenuText": "Smart select"}

    def GetCommands(self):
        return ("Sandwich_Bread",)

    def IsActive(self):
        return True


class _Workbench(Gui.Workbench):
    MenuText = "Sandwich"
    ToolTip = "Bread and filling"
    Icon = "/tmp/sandwich.svg"

    def __init__(self):
        super().__init__()
        self.lifecycle = []

    def Initialize(self):
        Gui.addCommand("Sandwich_Bread", self.bread)
        Gui.addCommand("Sandwich_Smart", self.group)
        self.appendToolbar("Sandwich Draw", ["Sandwich_Bread", "Separator"])
        self.appendMenu(["Sandwich", "Draw"], ["Sandwich_Bread"])
        self.appendMenu("Sandwich", ["Sandwich_Smart"])

    def Activated(self):
        self.lifecycle.append("activated")

    def Deactivated(self):
        self.lifecycle.append("deactivated")


class WorkbenchHostTests(unittest.TestCase):
    def setUp(self):
        # The registries are process-global; isolate each test.
        self._saved_workbenches = dict(Gui._WORKBENCHES)
        self._saved_commands = dict(Gui._COMMANDS)
        self._saved_active = Gui._ACTIVE_WORKBENCH
        self._saved_host = Gui._GUI_HOST
        Gui._WORKBENCHES.clear()
        Gui._ACTIVE_WORKBENCH = None
        Gui._GUI_HOST = None
        self.bread = _Command()
        self.group = _Group()
        self.workbench = _Workbench()
        self.workbench.bread = self.bread
        self.workbench.group = self.group

    def tearDown(self):
        Gui._WORKBENCHES.clear()
        Gui._WORKBENCHES.update(self._saved_workbenches)
        Gui._COMMANDS.clear()
        Gui._COMMANDS.update(self._saved_commands)
        Gui._ACTIVE_WORKBENCH = self._saved_active
        Gui._GUI_HOST = self._saved_host

    # -- registration -----------------------------------------------------------

    def test_a_workbench_registers_under_its_class_name(self):
        Gui.addWorkbench(self.workbench)
        self.assertEqual(list(Gui.listWorkbenches()), ["_Workbench"])
        self.assertIs(Gui.getWorkbench("_Workbench"), self.workbench)
        self.assertIsNone(Gui.getWorkbench("Nope"))
        # A second registration replaces rather than duplicating.
        Gui.addWorkbench(self.workbench)
        self.assertEqual(len(Gui.listWorkbenches()), 1)

    def test_only_a_workbench_can_be_added(self):
        with self.assertRaises(TypeError):
            Gui.addWorkbench(object())

    # -- the declaration tables -------------------------------------------------

    def test_activation_initialises_once_and_copies_the_declarations(self):
        Gui.addWorkbench(self.workbench)
        registered = len(Gui.listCommands())
        # Before activation the tables are empty: Initialize has not run.
        before = json.loads(ferrocad_shell.workbenches())
        self.assertEqual(before["active"], "")
        self.assertEqual(before["workbenches"][0]["toolbars"], [])
        self.assertFalse(before["workbenches"][0]["initialized"])

        answer = json.loads(
            ferrocad_shell.activate_workbench(json.dumps({"name": "_Workbench"}))
        )
        self.assertTrue(answer["ok"], answer)
        self.assertEqual(answer["active"], "_Workbench")
        entry = answer["workbenches"][0]
        self.assertTrue(entry["initialized"])
        self.assertEqual(entry["menu"], "Sandwich")
        self.assertEqual(entry["icon"], "/tmp/sandwich.svg")
        self.assertEqual(
            entry["toolbars"],
            [{"name": "Sandwich Draw", "commands": ["Sandwich_Bread", "Separator"]}],
        )
        self.assertEqual(
            [row["path"] for row in entry["menus"]], [["Sandwich", "Draw"], ["Sandwich"]]
        )

        # Initialize ran once: a second activation must not register again (that would
        # raise on the duplicate command id), so the registry grew by exactly the two
        # commands this workbench declares.
        Gui.activateWorkbench("_Workbench")
        self.assertEqual(len(Gui.listCommands()), registered + 2)

    def test_an_unknown_workbench_names_what_is_registered(self):
        Gui.addWorkbench(self.workbench)
        with self.assertRaises(KeyError) as caught:
            Gui.activateWorkbench("Nope")
        self.assertIn("_Workbench", str(caught.exception))
        answer = json.loads(
            ferrocad_shell.activate_workbench(json.dumps({"name": "Nope"}))
        )
        self.assertFalse(answer["ok"])
        self.assertIn("no workbench named", answer["error"])

    def test_the_workbench_lists_its_commands_without_separators_or_duplicates(self):
        Gui.addWorkbench(self.workbench)
        Gui.activateWorkbench("_Workbench")
        self.assertEqual(
            self.workbench.listCommands(), ["Sandwich_Bread", "Sandwich_Smart"]
        )

    # -- the lifecycle split ----------------------------------------------------

    def test_the_qt_half_runs_only_with_a_host(self):
        Gui.addWorkbench(self.workbench)
        Gui.activateWorkbench("_Workbench")
        self.assertEqual(self.workbench.lifecycle, [], "headless: Activated is skipped")

        class Host:
            """A adopted Qt main window, in miniature."""

        Gui._set_gui_host(Host())
        Gui._deactivate_active_workbench()
        Gui.activateWorkbench("_Workbench")
        self.assertEqual(self.workbench.lifecycle, ["activated"])

        Gui.activateWorkbench("_Workbench")  # already active: no second Activated
        self.assertEqual(self.workbench.lifecycle, ["activated"])
        Gui._deactivate_active_workbench()
        self.assertEqual(self.workbench.lifecycle, ["activated", "deactivated"])

    def test_get_main_window_follows_the_adopted_host(self):
        headless = Gui.getMainWindow()
        host = object()
        Gui._set_gui_host(host)
        try:
            self.assertIs(Gui.getMainWindow(), host)
        finally:
            Gui._set_gui_host(None)
        self.assertIs(Gui.getMainWindow(), headless)

    # -- command groups ---------------------------------------------------------

    def test_a_command_group_registers_without_activated(self):
        # `SmartSelectGroup` in SimpleCAD is exactly this shape: a toolbar dropdown.
        Gui.addCommand("Sandwich_Smart", self.group)
        self.assertIn("Sandwich_Smart", Gui.listCommands())

        listed = {entry["id"]: entry for entry in Gui._command_list()}
        self.assertTrue(listed["Sandwich_Smart"]["is_group"])
        self.assertEqual(listed["Sandwich_Smart"]["commands"], ["Sandwich_Bread"])

        # Running a group refuses by name instead of guessing the default member.
        answer = Gui._run("Sandwich_Smart")
        self.assertFalse(answer["ok"])
        self.assertIn("command group", answer["error"])

    def test_a_registration_without_either_half_is_refused(self):
        with self.assertRaises(TypeError):
            Gui.addCommand("Sandwich_Empty", object())


class SimpleCadWorkbenchTests(unittest.TestCase):
    """The real thing: the sibling checkout's own five-line ``InitGui.py``."""

    def setUp(self):
        if not os.path.isdir(SIMPLECAD):
            self.skipTest("no ../simplecad checkout (set SIMPLECAD_DIR)")
        if SIMPLECAD not in sys.path:
            sys.path.insert(0, SIMPLECAD)
        self._saved_active = Gui._ACTIVE_WORKBENCH
        self._saved_host = Gui._GUI_HOST
        Gui._ACTIVE_WORKBENCH = None
        Gui._GUI_HOST = None

    def tearDown(self):
        Gui._ACTIVE_WORKBENCH = self._saved_active
        Gui._GUI_HOST = self._saved_host

    def test_the_checkout_declares_its_workbench(self):
        import InitGui  # noqa: F401  (registers SimpleCADWorkbench)

        self.assertIn("SimpleCADWorkbench", Gui.listWorkbenches())
        answer = json.loads(
            ferrocad_shell.activate_workbench(
                json.dumps({"name": "SimpleCADWorkbench"})
            )
        )
        self.assertTrue(answer["ok"], answer)
        self.assertEqual(answer["active"], "SimpleCADWorkbench")

        entry = answer["workbenches"][0]
        self.assertEqual(entry["menu"], "SimpleCAD")
        names = [row["name"] for row in entry["toolbars"]]
        self.assertEqual(
            names,
            [
                "SimpleCAD Draw",
                "SimpleCAD Modify",
                "SimpleCAD Surface",
                "SimpleCAD Inspect",
                "SimpleCAD Mesh",
            ],
        )
        # Every row references a registered command, and every command id is in the
        # `<Workbench>_<Action>` namespace a toolbar expects.
        registered = set(Gui.listCommands())
        for row in entry["toolbars"] + entry["menus"]:
            label = row.get("name") or "/".join(row.get("path", []))
            for command in row["commands"]:
                if command == "Separator":
                    continue
                self.assertIn(command, registered, label)
                self.assertTrue(command.startswith("SimpleCAD_"), command)
        # The group in the Inspect toolbar is a dropdown, not an action.
        self.assertIn("SimpleCAD_SmartSelect", registered)
        grouped = {c["id"]: c for c in Gui._command_list()}["SimpleCAD_SmartSelect"]
        self.assertTrue(grouped["is_group"])
        self.assertTrue(grouped["commands"])


if __name__ == "__main__":
    unittest.main()
