"""Tests for ``App_ToggleGizmo``, the header's gizmo show/hide command.

``ferrocad_shell`` registers the command; it owns ``FerroCAD.Gizmo.enabled``
and returns the partial ``{"view": {"gizmo": {"enabled": …}}}`` request the
shell applies. ``IsChecked`` reads the same store, so the header pill's dot and
the popover row follow the drawn handles. The suite is headless
(``FreeCAD.GuiUp == 0``): the command still validates, stores and returns its
request.
"""

import json
import unittest
from unittest import mock

import FreeCAD
import FerroCAD
import FreeCADGui
import ferrocad_shell  # noqa: F401  (registers App_ToggleGizmo)

COMMAND = "App_ToggleGizmo"
PREFERENCES = "User parameter:BaseApp/Preferences/Mods/gizmo/ferrocad"


def clear_preferences():
    FreeCAD.ParamGet(PREFERENCES).Clear()


def command_entry():
    return next(
        entry for entry in FreeCADGui._command_list() if entry["id"] == COMMAND
    )


class GizmoToggleCommandTests(unittest.TestCase):
    """``App_ToggleGizmo`` writes ``FerroCAD.Gizmo.enabled``."""

    def setUp(self):
        self.gizmo = FerroCAD.Gizmo
        self.gizmo.reset()
        clear_preferences()

    def tearDown(self):
        self.gizmo.reset()
        clear_preferences()

    def test_the_command_registers_checkable_on_the_gizmo_surface(self):
        self.assertIn(COMMAND, FreeCADGui.listCommands())

        # `_command_list` is the upstream-shaped view; the shell bridge adds the
        # surface and `checkable` fields the header reads.
        command = next(
            entry
            for entry in json.loads(ferrocad_shell.commands())
            if entry["id"] == COMMAND
        )
        self.assertEqual(command["menu"], "Gizmo")
        self.assertEqual(command["tooltip"], "Show the transform gizmo")
        self.assertEqual(command["group"], "View")
        self.assertEqual(command["cmd_type"], "ForView")
        self.assertEqual(command["surface"], "gizmo")
        self.assertTrue(command["enabled"])
        self.assertTrue(command["checkable"], "a toggle row keeps the popover open")
        self.assertTrue(command["checked"], "the gizmo is on by default")
        self.assertFalse(command["dev_only"], "the header shows it")

    def test_the_command_flips_enabled_and_returns_the_gizmo_view(self):
        self.assertTrue(self.gizmo.enabled)

        request = FreeCADGui.runCommand(COMMAND)
        self.assertFalse(self.gizmo.enabled)
        self.assertEqual(request, {"view": {"gizmo": {"enabled": False}}})

        request = FreeCADGui.runCommand(COMMAND)
        self.assertTrue(self.gizmo.enabled)
        self.assertEqual(request, {"view": {"gizmo": {"enabled": True}}})

    def test_the_check_mark_follows_the_semantic_store(self):
        self.gizmo.enabled = False
        self.assertFalse(command_entry()["checked"])

        self.gizmo.enabled = True
        self.assertTrue(command_entry()["checked"])

    def test_without_the_gizmo_api_the_command_reports_instead_of_raising(self):
        with mock.patch.object(FerroCAD, "Gizmo", None):
            self.assertFalse(command_entry()["checked"])
            result = FreeCADGui._run(COMMAND)

        self.assertTrue(result["ok"], result)
        self.assertIn("unavailable", result["status"])
        self.assertNotIn("view", result)
