"""Tests for view and tool requests (A5) and the built-in grid command.

A `{"view": ...}` request is presentation-only; the shell applies it and mirrors the
result to `FreeCADGui.ViewState`, which a checkable command reads in `IsChecked`.
A `{"tool": id}` request names a Rust tool the edition registered. See
`docs/actions-and-tools.md` §7.
"""

import json
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import FreeCADGui

import ferrocad_shell  # noqa: F401  (registers App_ToggleGrid)


class ViewAndToolTests(unittest.TestCase):
    def setUp(self):
        FreeCADGui.ViewState.grid = False
        FreeCADGui.ViewState.projection = "persp"
        self.names = []

    def tearDown(self):
        for name in self.names:
            FreeCADGui.removeCommand(name)
        FreeCADGui.ViewState.grid = False
        FreeCADGui.ViewState.projection = "persp"

    def add(self, name, command):
        FreeCADGui.addCommand(name, command)
        self.names.append(name)

    def entry(self, name):
        return next(c for c in FreeCADGui._command_list() if c["id"] == name)

    def test_the_view_state_defaults_to_no_grid_and_perspective(self):
        self.assertFalse(FreeCADGui.ViewState.grid)
        self.assertEqual(FreeCADGui.ViewState.projection, "persp")

    def test_the_grid_command_is_checkable_and_closes_the_loop(self):
        entry = self.entry("App_ToggleGrid")
        self.assertFalse(entry["checked"])
        self.assertEqual(entry["cmd_type"], "ForView")
        self.assertEqual(entry["group"], "View")

        result = FreeCADGui._run("App_ToggleGrid")
        self.assertTrue(result["ok"])
        self.assertEqual(result["view"], {"grid": True})

        # The shell applies the request and mirrors the state back; the check mark
        # then reflects it.
        ferrocad_shell.set_view_state(json.dumps({"grid": True, "projection": "persp"}))
        self.assertTrue(self.entry("App_ToggleGrid")["checked"])

    def test_a_tool_request_survives_the_payload(self):
        class _Tool:
            def Activated(self):
                return {"tool": "some_tool"}

        self.add("Test_Tool", _Tool())
        result = FreeCADGui._run("Test_Tool")
        self.assertEqual(result["tool"], "some_tool")
        json.dumps(result)  # the callable (if any) must never reach the payload

    def test_a_view_request_and_a_status_can_coexist(self):
        class _Both:
            def Activated(self):
                return {"status": "toggled", "view": {"projection": "ortho"}}

        self.add("Test_Both", _Both())
        result = FreeCADGui._run("Test_Both")
        self.assertEqual(result["status"], "toggled")
        self.assertEqual(result["view"], {"projection": "ortho"})


if __name__ == "__main__":
    unittest.main()
