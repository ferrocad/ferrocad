"""Tests for view and tool requests (A5) and the built-in grid command.

A `{"view": ...}` request is presentation-only; the shell applies it and mirrors the
result to `FerroCADGui.ViewState`, which a checkable command reads in `IsChecked`.
A `{"tool": id}` request names a Rust tool the edition registered. See
`docs/developer/workbenches/actions-and-tools.md` §7.
"""

import json
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import FreeCADGui
import FerroCAD
import FerroCADGui

import ferrocad_shell  # noqa: F401  (registers App_ToggleGrid)


class ViewAndToolTests(unittest.TestCase):
    def setUp(self):
        FerroCAD.Grid.reset()
        FerroCADGui.ViewState.grid = False
        FerroCADGui.ViewState.projection = "persp"
        self.names = []

    def tearDown(self):
        for name in self.names:
            FreeCADGui.removeCommand(name)
        FerroCADGui.ViewState.grid = False
        FerroCADGui.ViewState.projection = "persp"
        FerroCAD.Grid.reset()

    def add(self, name, command):
        FreeCADGui.addCommand(name, command)
        self.names.append(name)

    def entry(self, name):
        return next(c for c in FreeCADGui._command_list() if c["id"] == name)

    def test_the_view_state_defaults_to_no_grid_and_perspective(self):
        self.assertFalse(FerroCADGui.ViewState.grid)
        self.assertEqual(FerroCADGui.ViewState.projection, "persp")

    def test_the_grid_command_is_checkable_and_closes_the_loop(self):
        entry = self.entry("App_ToggleGrid")
        self.assertFalse(entry["checked"])
        self.assertEqual(entry["cmd_type"], "ForView")
        self.assertEqual(entry["group"], "View")

        result = FreeCADGui._run("App_ToggleGrid")
        self.assertTrue(result["ok"])
        self.assertTrue(result["view"]["grid"])
        self.assertTrue(FerroCAD.Grid.enabled, "the one switch is on")
        # ``FerroCAD.Grid.apply()`` is the base of the request, so the appearance
        # fields travel with the toggle.
        self.assertIn("grid_minor", result["view"])

        # The shell applies the request and mirrors the state back; the check mark
        # then reflects it.
        ferrocad_shell.set_view_state(json.dumps({"grid": True, "projection": "persp"}))
        self.assertTrue(self.entry("App_ToggleGrid")["checked"])

        # A second toggle returns to the start state (switch, request and mark).
        result = FreeCADGui._run("App_ToggleGrid")
        self.assertFalse(result["view"]["grid"])
        self.assertFalse(FerroCAD.Grid.enabled)
        ferrocad_shell.set_view_state(
            json.dumps({"grid": False, "projection": "persp"})
        )
        self.assertFalse(self.entry("App_ToggleGrid")["checked"])

    def test_the_toggle_owns_the_semantic_switch_not_the_mirror(self):
        # A stale mirror must not decide the next state: ``FerroCAD.Grid.enabled``
        # is the one switch.
        FerroCAD.Grid.enabled = True
        FerroCADGui.ViewState.grid = False  # a mirror that has not caught up

        result = FreeCADGui._run("App_ToggleGrid")

        self.assertFalse(result["view"]["grid"])
        self.assertFalse(FerroCAD.Grid.enabled)

    def test_a_tool_request_survives_the_payload(self):
        class _Tool:
            def Activated(self):
                return {"tool": "some_tool"}

        self.add("Test_Tool", _Tool())
        result = FreeCADGui._run("Test_Tool")
        self.assertEqual(result["tool"], "some_tool")
        json.dumps(result)  # the callable (if any) must never reach the payload

    def test_a_workplane_stem_directive_survives_the_payload(self):
        # The offset HUD asks for the viewport's `+N` stem with a top-level
        # `workplane_stem`, not a dialog key: the facade must forward it beside the
        # spec. The older `gizmo` spelling still travels for a shell that predates it.
        result = FreeCADGui._run("App_OffsetWorkplane")
        self.assertTrue(result["ok"])
        self.assertTrue(result["workplane_stem"])
        self.assertEqual(result["gizmo"], "workplane_offset")
        self.assertEqual(result["dialog"]["preview"], "workplane_offset")
        self.assertTrue(result["token"])
        json.dumps(result)  # the callable must never reach the payload

    def test_a_legacy_gizmo_request_still_survives_the_payload(self):
        class _Legacy:
            def Activated(self):
                return {"gizmo": "workplane_offset"}

        self.add("Test_WorkplaneStemLegacy", _Legacy())
        result = FreeCADGui._run("Test_WorkplaneStemLegacy")
        self.assertEqual(result["gizmo"], "workplane_offset")
        self.assertNotIn("workplane_stem", result)

    def test_a_view_request_and_a_status_can_coexist(self):
        class _Both:
            def Activated(self):
                return {"status": "toggled", "view": {"projection": "ortho"}}

        self.add("Test_Both", _Both())
        result = FreeCADGui._run("Test_Both")
        self.assertEqual(result["status"], "toggled")
        self.assertEqual(result["view"], {"projection": "ortho"})

    def test_the_standard_workplane_rows_name_planes_not_views(self):
        """`YZ` is the **Side** plane; `Right` is a camera view, not a plane.

        Upstream `WorkingPlane.set_to_side` is the preset, and the YZ plane is `x = 0`
        — it is not "the plane on the right of the model". Calling the row `(Right)`
        conflated the plane preset with `Std_ViewRight`, which reads as if the plane
        had moved to the right of the geometry.
        """
        entries = {entry["id"]: entry for entry in json.loads(ferrocad_shell.commands())}
        self.assertEqual(entries["App_WorkplaneXY"]["menu"], "World XY (Top)")
        self.assertEqual(entries["App_WorkplaneXZ"]["menu"], "World XZ (Front)")
        self.assertEqual(entries["App_WorkplaneYZ"]["menu"], "World YZ (Side)")
        self.assertEqual(entries["App_ViewWorkplane"]["menu"], "Align camera to WP")


if __name__ == "__main__":
    unittest.main()
