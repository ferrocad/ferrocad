"""Tests for the native Draft control surface (Q4).

``FreeCADGui.draft`` provides the upstream-shaped ``Snapper`` and
``draftToolBar`` singletons over the Rust-free snap maths in
``ferrocad_stage``, plus the six checkable ``Draft_Snap_*`` commands, the hint
channel and the headless main-window seam.

The stage-dependent cases skip cleanly when ``ferrocad_stage`` has not been
built; the command registry, hint helpers and task-panel state do not need it.
"""

import contextlib
import io
import unittest
from unittest import mock

import FreeCAD
import FreeCADGui
from FreeCADGui import draft

try:
    import ferrocad_stage as stage
except ImportError:  # the Rust harness may register it in sys.modules instead
    stage = None

needs_stage = unittest.skipIf(stage is None, "the ferrocad_stage extension is not built")

SNAP_COMMANDS = [
    "Draft_Snap_Grid",
    "Draft_Snap_Endpoint",
    "Draft_Snap_Midpoint",
    "Draft_Snap_Center",
    "Draft_Snap_Origin",
    "Draft_Snap_Vertex",
]


class DraftTestCase(unittest.TestCase):
    def setUp(self):
        draft.Snapper.reset()
        self.toolbar = draft.draftToolBar
        self.toolbar.sourceCmd = None
        self.toolbar.isTaskOn = False
        self.toolbar.autogroup = None
        self.toolbar.last_point = None

    def tearDown(self):
        draft.Snapper.reset()
        self.toolbar.sourceCmd = None
        self.toolbar.isTaskOn = False


# ---------------------------------------------------------------------------
# The checkable snap bar
# ---------------------------------------------------------------------------


class SnapCommandTests(DraftTestCase):
    def test_the_toolbar_and_registry_agree(self):
        self.assertEqual(list(draft.SNAP_TOOLBAR), SNAP_COMMANDS)
        self.assertEqual(draft.Snapper.get_snap_toolbar(), SNAP_COMMANDS)
        for command_id in SNAP_COMMANDS:
            self.assertIn(command_id, FreeCADGui.listCommands())

    def test_supported_modes_toggle_checked_and_mask(self):
        expected = {
            "Draft_Snap_Grid": "Grid",
            "Draft_Snap_Endpoint": "Endpoint",
            "Draft_Snap_Midpoint": "Midpoint",
            "Draft_Snap_Center": "Center",
            "Draft_Snap_Origin": "Origin",
            "Draft_Snap_Vertex": "Vertex",
        }
        self.assertEqual(draft.supported_modes(), list(expected.values()))

        for command_id, mode in expected.items():
            with self.subTest(mode=mode):
                command = FreeCADGui._COMMANDS[command_id]
                resources = command.GetResources()
                self.assertTrue(resources["MenuText"])
                self.assertTrue(resources["ToolTip"])
                self.assertTrue(command.IsActive())
                self.assertTrue(command.IsChecked(), "the reset mask enables every mode")
                self.assertIn(mode, draft.Snapper.mask)

                command.Activated()
                self.assertFalse(command.IsChecked())
                self.assertNotIn(mode, draft.Snapper.mask)
                self.assertFalse(draft.Snapper.isEnabled(mode))

                command.Activated()
                self.assertTrue(command.IsChecked())
                self.assertIn(mode, draft.Snapper.mask)

                # An explicit status sets rather than toggles.
                command.Activated(False)
                self.assertFalse(command.IsChecked())
                command.Activated(True)
                self.assertTrue(command.IsChecked())

    def test_active_follows_the_global_switch(self):
        command = FreeCADGui._COMMANDS["Draft_Snap_Grid"]
        self.assertTrue(command.IsActive())
        self.assertTrue(command.IsChecked())

    def test_every_draft_only_mode_raises_and_names_itself(self):
        self.assertEqual(
            draft.unsupported_modes(),
            [
                "Extension",
                "Parallel",
                "Perpendicular",
                "Intersection",
                "Angle",
                "Special",
                "Dimensions",
                "Ortho",
                "Lock",
            ],
        )
        for mode in draft.unsupported_modes():
            with self.subTest(mode=mode):
                with self.assertRaises(NotImplementedError) as caught:
                    draft.Snapper.toggle_snap(mode)
                self.assertIn(mode, str(caught.exception))
                with self.assertRaises(NotImplementedError):
                    draft.Snapper.enable(mode)
                with self.assertRaises(NotImplementedError):
                    draft.Snapper.mask = [mode]

    def test_unknown_modes_are_rejected(self):
        with self.assertRaises(ValueError):
            draft.Snapper.toggle_snap("Nope")
        with self.assertRaises(TypeError):
            draft.Snapper.mask = [17]


# ---------------------------------------------------------------------------
# Snapping through SnapContext
# ---------------------------------------------------------------------------


@needs_stage
class SnapperSnapTests(DraftTestCase):
    def assertPoint(self, actual, expected, places=4):
        self.assertEqual(len(actual), 3)
        for got, want in zip(actual, expected):
            self.assertAlmostEqual(got, want, places=places)

    def test_the_world_grid_snaps_and_remembers_the_snap(self):
        draft.Snapper.mask = ["Grid"]
        draft.Snapper.affinity = 0.0

        point = draft.Snapper.snap((1.2, 2.1, 3.0))

        self.assertPoint(point, (1.0, 2.0, 0.0))
        self.assertEqual(draft.Snapper.snapInfo.kind, stage.SnapKind.Grid)
        self.assertEqual(draft.Snapper.snapInfo.local, (1.0, 2.0))
        self.assertFalse(draft.Snapper.snapInfo.major)

    def test_a_disabled_grid_returns_the_projected_point(self):
        draft.Snapper.mask = ["Grid"]
        draft.Snapper.setGrid(False)

        point = draft.Snapper.snap((1.2, 2.1, 3.0))

        self.assertPoint(point, (1.2, 2.1, 0.0))
        self.assertEqual(draft.Snapper.snapInfo.kind, stage.SnapKind.Point)

    def test_endpoint_and_vertex_candidates_win_within_affinity(self):
        candidate = stage.SnapCandidate((2.4, 2.4, 0.0), stage.SnapKind.Vertex)
        for mode in ("Endpoint", "Vertex"):
            with self.subTest(mode=mode):
                draft.Snapper.reset()
                draft.Snapper.mask = [mode]
                draft.Snapper.affinity = 0.5

                point = draft.Snapper.snap((2.1, 2.1, 0.0), [candidate])

                self.assertPoint(point, (2.4, 2.4, 0.0))
                self.assertEqual(draft.Snapper.snapInfo.kind, stage.SnapKind.Vertex)

    def test_edge_midpoint_face_centre_and_origin_candidates(self):
        cases = [
            ("Midpoint", stage.SnapKind.EdgeMid, (1.0, 2.0, 0.0), (1.2, 2.1, 0.0)),
            ("Center", stage.SnapKind.Centre, (3.0, 3.0, 0.0), (3.1, 3.1, 0.0)),
            ("Origin", stage.SnapKind.Origin, (0.0, 0.0, 0.0), (0.2, 0.2, 0.0)),
        ]
        for mode, kind, target, cursor in cases:
            with self.subTest(mode=mode):
                draft.Snapper.reset()
                draft.Snapper.mask = [mode]
                draft.Snapper.affinity = 0.5
                candidate = stage.SnapCandidate(target, kind)

                point = draft.Snapper.snap(cursor, [candidate])

                self.assertPoint(point, target)
                self.assertEqual(draft.Snapper.snapInfo.kind, kind)

    def test_a_candidate_outside_affinity_loses_to_the_grid(self):
        draft.Snapper.mask = ["Grid", "Endpoint"]
        draft.Snapper.affinity = 0.1
        candidate = stage.SnapCandidate((2.4, 2.4, 0.0), stage.SnapKind.Vertex)

        point = draft.Snapper.snap((2.1, 2.1, 0.0), [candidate])

        self.assertPoint(point, (2.0, 2.0, 0.0))
        self.assertEqual(draft.Snapper.snapInfo.kind, stage.SnapKind.Grid)

    def test_a_rotated_workplane_snaps_in_its_own_coordinates(self):
        root = 0.5**0.5
        plane = stage.Workplane((10.0, 0.0, 5.0), (root, root, 0.0), (-root, root, 0.0))
        draft.Snapper.setWorkplane(plane)
        draft.Snapper.mask = ["Grid"]
        draft.Snapper.affinity = 0.0

        on_plane = plane.point(2.3, 3.2)
        off_plane = (on_plane[0], on_plane[1], on_plane[2] + 4.0)
        point = draft.Snapper.snap(off_plane)

        self.assertPoint(point, plane.point(2.0, 3.0))
        self.assertAlmostEqual(draft.Snapper.snapInfo.local[0], 2.0, places=6)
        self.assertAlmostEqual(draft.Snapper.snapInfo.local[1], 3.0, places=6)

    def test_set_angle_rotates_the_grid_about_the_normal(self):
        draft.Snapper.mask = ["Grid"]
        draft.Snapper.affinity = 0.0
        rotated = draft.Snapper.setAngle(90)

        self.assertAlmostEqual(rotated.u[0], 0.0, places=9)
        self.assertAlmostEqual(rotated.u[1], 1.0, places=9)
        self.assertAlmostEqual(rotated.v[0], -1.0, places=9)
        self.assertAlmostEqual(rotated.v[1], 0.0, places=9)

        # The point (-3.2, -2.1) in the rotated frame is (2, -3) on the grid.
        point = draft.Snapper.snap((2.1, -3.2, 0.0))
        self.assertPoint(point, (2.0, -3.0, 0.0))
        self.assertAlmostEqual(draft.Snapper.snapInfo.local[0], -3.0, places=6)
        self.assertAlmostEqual(draft.Snapper.snapInfo.local[1], -2.0, places=6)

    def test_hold_points_are_vertex_candidates(self):
        draft.Snapper.mask = ["Vertex"]
        draft.Snapper.affinity = 0.5
        draft.Snapper.addHoldPoint((5.0, 6.0, 0.0))

        point = draft.Snapper.snap((5.1, 6.1, 0.0))

        self.assertPoint(point, (5.0, 6.0, 0.0))
        self.assertEqual(draft.Snapper.snapInfo.kind, stage.SnapKind.Vertex)


# ---------------------------------------------------------------------------
# Point sessions
# ---------------------------------------------------------------------------


@needs_stage
class SnapperSessionTests(DraftTestCase):
    def test_get_point_returns_immediately_and_pick_calls_once(self):
        calls = []

        result = draft.Snapper.getPoint(lambda point, tag: calls.append((point, tag)), "tag")

        self.assertIsNone(result)
        self.assertTrue(draft.Snapper.running)
        self.assertEqual(calls, [])

        snapped = draft.pick((1.2, 2.1, 0.0))

        self.assertEqual(snapped, (1.0, 2.0, 0.0))
        self.assertEqual(calls, [((1.0, 2.0, 0.0), "tag")])
        self.assertFalse(draft.Snapper.running)

        # The callback is cleared before it runs, so a second pick cannot repeat it.
        draft.pick((4.2, 4.2, 0.0))
        self.assertEqual(len(calls), 1)

    def test_pick_without_a_request_still_returns_the_snapped_point(self):
        self.assertEqual(draft.pick((1.2, 2.1, 0.0)), (1.0, 2.0, 0.0))

    def test_escape_drops_the_request_without_invoking_it(self):
        calls = []
        draft.Snapper.getPoint(calls.append)

        dropped = draft.escape()

        self.assertTrue(dropped)
        self.assertEqual(calls, [])
        self.assertFalse(draft.Snapper.running)

    def test_a_second_request_replaces_the_first_and_reports_it(self):
        first, second = [], []
        draft.Snapper.getPoint(first.append)
        errors = io.StringIO()
        with contextlib.redirect_stderr(errors):
            draft.Snapper.getPoint(second.append)

        self.assertIn("replacing", errors.getvalue())
        draft.pick((2.2, 3.2, 0.0))
        self.assertEqual(first, [])
        self.assertEqual(second, [(2.0, 3.0, 0.0)])

    def test_get_point_needs_a_callable(self):
        with self.assertRaises(TypeError):
            draft.Snapper.getPoint(None)

    def test_set_select_mode_and_off_reset_the_interaction(self):
        draft.Snapper.setSelectMode(True)
        self.assertTrue(draft.Snapper.selectMode)
        draft.Snapper.getPoint(lambda point: None)

        draft.Snapper.off()

        self.assertFalse(draft.Snapper.selectMode)
        self.assertFalse(draft.Snapper.running)
        self.assertIsNone(draft.Snapper.basepoint)
        self.assertIsNone(draft.Snapper.constraintAxis)


# ---------------------------------------------------------------------------
# Hints, input values and command helpers
# ---------------------------------------------------------------------------


class HintAndHelperTests(DraftTestCase):
    def test_hint_manager_shows_and_hides(self):
        text = FreeCADGui.HintManager.show("Click to place", "Shift to constrain")
        self.assertEqual(text, "Click to place\nShift to constrain")
        self.assertEqual(
            FreeCADGui.HintManager.hints, ("Click to place", "Shift to constrain")
        )
        self.assertEqual(FreeCADGui.draft.hint, text)
        self.assertEqual(FreeCADGui.HintManager.last, "Shift to constrain")

        FreeCADGui.HintManager.hide()
        self.assertEqual(FreeCADGui.HintManager.hints, ())
        self.assertEqual(FreeCADGui.HintManager.hint, "")
        self.assertEqual(FreeCADGui.draft.hint, "")
        self.assertIsNone(FreeCADGui.HintManager.last)

    def test_input_hint_is_a_value_object(self):
        hint = FreeCADGui.InputHint("Move the cursor", 3, "px")
        self.assertEqual(hint.text, "Move the cursor")
        self.assertEqual(hint.args, (3, "px"))
        self.assertIn("InputHint", repr(hint))
        self.assertIn("Move the cursor", repr(hint))
        self.assertEqual(hint, FreeCADGui.InputHint("Move the cursor", 3, "px"))

    def test_user_input_sentinels_are_distinct(self):
        inputs = [
            FreeCADGui.UserInput.MouseLeft,
            FreeCADGui.UserInput.MouseRight,
            FreeCADGui.UserInput.KeyEscape,
            FreeCADGui.UserInput.KeyEnter,
        ]
        self.assertEqual(len(set(map(id, inputs))), 4)
        self.assertEqual(repr(FreeCADGui.UserInput.MouseLeft), "UserInput.MouseLeft")

    def test_add_module_records_a_name_without_importing(self):
        FreeCADGui._PENDING_MODULES.clear()
        try:
            FreeCADGui.addModule("Draft")
            FreeCADGui.addModule("Draft")
            self.assertEqual(FreeCADGui._PENDING_MODULES, ["Draft"])
            self.assertEqual(FreeCADGui._take_modules(), ["Draft"])
            self.assertEqual(FreeCADGui._PENDING_MODULES, [])
            with self.assertRaises(ValueError):
                FreeCADGui.addModule("")
        finally:
            FreeCADGui._PENDING_MODULES.clear()

    def test_do_command_evaluates_in_the_freecad_namespace(self):
        name = "_ferrocad_draft_probe"
        FreeCAD.__dict__.pop(name, None)
        try:
            FreeCADGui.doCommand(f"{name} = 7")
            self.assertEqual(getattr(FreeCAD, name), 7)
            with self.assertRaises(RuntimeError) as caught:
                FreeCADGui.doCommand("raise ValueError('broken')")
            self.assertIn("doCommand", str(caught.exception))
            self.assertIn("broken", str(caught.exception))
        finally:
            FreeCAD.__dict__.pop(name, None)

    def test_do_command_prefers_a_freecad_run_command_hook(self):
        with mock.patch.object(
            FreeCAD, "runCommand", return_value="ran", create=True
        ) as runner:
            self.assertEqual(FreeCADGui.doCommand("pass"), "ran")
        runner.assert_called_once_with("pass")


# ---------------------------------------------------------------------------
# The headless main window
# ---------------------------------------------------------------------------


class MainWindowTests(DraftTestCase):
    def test_get_main_window_returns_a_view_with_the_event_protocol(self):
        window = FreeCADGui.getMainWindow()
        self.assertIs(window, FreeCADGui.getMainWindow())
        view = window.getActiveWindow()
        self.assertIsNotNone(view)

        for name in (
            "getSceneGraph",
            "addEventCallback",
            "removeEventCallback",
            "fitAll",
            "getPoint",
            "redraw",
        ):
            self.assertTrue(callable(getattr(view, name)), name)

        # ``None`` without Coin, a real node with it; either way the call works.
        view.getSceneGraph()

        callback = lambda event: None  # noqa: E731 - a stand-in event callback
        view.addEventCallback("mouse", callback)
        self.assertIn(callback, view._event_callbacks["mouse"])
        self.assertTrue(view.removeEventCallback("mouse", callback))
        self.assertFalse(view.removeEventCallback("mouse", callback))

        self.assertTrue(view.fitAll())
        self.assertTrue(view.redraw())

    def test_a_view_point_request_uses_the_snapper_session(self):
        view = FreeCADGui.getMainWindow().getActiveWindow()
        calls = []
        view.getPoint(calls.append, "extra")
        self.assertTrue(draft.Snapper.running)

        draft.escape()

        self.assertEqual(calls, [])
        self.assertFalse(draft.Snapper.running)

    def test_a_point_request_without_a_callback_is_rejected(self):
        with self.assertRaises(RuntimeError):
            FreeCADGui.getMainWindow().getActiveWindow().getPoint()


# ---------------------------------------------------------------------------
# The task-panel adapter
# ---------------------------------------------------------------------------


class _SourceCommand:
    def __init__(self):
        self.finished = []

    def finish(self, cont=None):
        self.finished.append(cont)


class _NoArgSourceCommand:
    def __init__(self):
        self.finished = 0

    def finish(self):
        self.finished += 1


class _ConstraintProvider:
    def __init__(self):
        self.calls = []

    def has_point_constraints(self):
        return True

    def constrain_point(self, point, lastpoint):
        self.calls.append((point, lastpoint))
        return (0.0, 0.0, 0.0)


class DraftToolBarTests(DraftTestCase):
    def test_the_ui_placeholders_have_their_upstream_shape(self):
        toolbar = self.toolbar
        self.assertIs(toolbar.ui, toolbar)
        self.assertTrue(toolbar.mouse)
        self.assertIsNone(toolbar.sourceCmd)
        self.assertFalse(toolbar.isTaskOn)
        self.assertFalse(toolbar.isConstructionMode)
        self.assertIsNone(toolbar.autogroup)
        self.assertFalse(toolbar.globalMode)
        self.assertIsNone(toolbar.wplabel)
        self.assertIsNone(toolbar.textValue)
        self.assertTrue(toolbar.makeFaceMode)
        self.assertFalse(toolbar.continueCmd)
        self.assertFalse(toolbar.loadedPreferences)
        self.assertIsNone(toolbar.tray)
        self.assertIsNone(toolbar.taskUi)

    def test_activated_and_off_ui_manage_the_task_state(self):
        self.toolbar.sourceCmd = _SourceCommand()

        self.toolbar.Activated()
        self.assertTrue(self.toolbar.isTaskOn)
        self.assertIs(draft.Snapper.ui, self.toolbar)

        self.toolbar.offUi()
        self.assertFalse(self.toolbar.isTaskOn)
        self.assertIsNone(self.toolbar.sourceCmd)
        self.assertIsNone(draft.Snapper.pointConstraintProvider)

    def test_deactivated_finishes_the_source_command(self):
        command = _SourceCommand()
        self.toolbar.sourceCmd = command
        self.toolbar.isTaskOn = True

        self.toolbar.Deactivated()

        self.assertEqual(command.finished, [False])
        self.assertFalse(self.toolbar.isTaskOn)
        self.assertIsNone(self.toolbar.sourceCmd)

    def test_escape_ends_snapping_and_finishes_the_command(self):
        command = _SourceCommand()
        self.toolbar.sourceCmd = command
        self.toolbar.isTaskOn = True
        calls = []
        draft.Snapper.getPoint(calls.append)

        self.toolbar.escape()

        self.assertEqual(command.finished, [False])
        self.assertEqual(calls, [])
        self.assertFalse(draft.Snapper.running)
        self.assertFalse(self.toolbar.isTaskOn)
        self.assertIsNone(self.toolbar.sourceCmd)

    def test_escape_accepts_the_older_finish_signature(self):
        command = _NoArgSourceCommand()
        self.toolbar.sourceCmd = command

        self.toolbar.escape()

        self.assertEqual(command.finished, 1)

    def test_set_auto_group(self):
        self.assertEqual(self.toolbar.setAutoGroup("Layer"), "Layer")
        self.assertEqual(self.toolbar.autogroup, "Layer")
        self.assertIsNone(self.toolbar.setAutoGroup(None))
        self.assertIsNone(self.toolbar.autogroup)

    def test_the_style_button_is_not_a_native_widget(self):
        with self.assertRaises(NotImplementedError):
            self.toolbar.setStyleButton()

    @needs_stage
    def test_validate_point_applies_the_current_constraint(self):
        draft.Snapper.constrain((10.0, 5.0, 0.0), basepoint=(0.0, 0.0, 0.0), axis="x")
        try:
            self.assertEqual(self.toolbar.validatePoint((4.0, 7.0, 0.0)), (4.0, 0.0, 0.0))
        finally:
            draft.Snapper.unconstrain()

        self.assertEqual(self.toolbar.validatePoint((4.0, 7.0, 0.0)), (4.0, 7.0, 0.0))
        self.assertTrue(self.toolbar.validatePoint())

    @needs_stage
    def test_validate_point_consults_the_constraint_provider(self):
        provider = _ConstraintProvider()
        draft.Snapper.setPointConstraintProvider(provider)
        try:
            self.assertEqual(self.toolbar.validatePoint((3.0, 4.0, 0.0)), (0.0, 0.0, 0.0))
        finally:
            draft.Snapper.clearPointConstraintProvider(provider)
        self.assertEqual(len(provider.calls), 1)
        self.assertIsNone(draft.Snapper.pointConstraintProvider)


# ---------------------------------------------------------------------------
# The compatibility stubs
# ---------------------------------------------------------------------------


class SnapperCompatibilityTests(DraftTestCase):
    @needs_stage
    def test_set_trackers_refreshes_and_returns_the_context(self):
        context = draft.Snapper.setTrackers()
        self.assertEqual(context.minor, 1.0)
        self.assertEqual(context.major, 10.0)

    def test_stubs_without_a_native_meaning_raise_clearly(self):
        for call in (draft.Snapper.restack, draft.Snapper.cycleSnapObject):
            with self.subTest(call=call.__name__):
                with self.assertRaises(NotImplementedError) as caught:
                    call()
                self.assertIn(call.__name__, str(caught.exception))

    def test_add_hold_point_needs_a_point(self):
        with self.assertRaises(ValueError):
            draft.Snapper.addHoldPoint()

    @needs_stage
    def test_constrain_and_unconstrain_round_trip(self):
        point = draft.Snapper.constrain((10.0, 5.0, 0.0), basepoint=(0.0, 0.0, 0.0))
        self.assertAlmostEqual(point[0], 10.0)
        self.assertAlmostEqual(point[1], 0.0)
        self.assertEqual(draft.Snapper.basepoint, (0.0, 0.0, 0.0))
        self.assertIsNotNone(draft.Snapper.constraintAxis)

        draft.Snapper.unconstrain()
        self.assertIsNone(draft.Snapper.basepoint)
        self.assertIsNone(draft.Snapper.constraintAxis)

    def test_a_point_constraint_provider_can_be_cleared_by_identity(self):
        first, second = object(), object()
        draft.Snapper.setPointConstraintProvider(first)
        draft.Snapper.clearPointConstraintProvider(second)
        self.assertIs(draft.Snapper.pointConstraintProvider, first)
        draft.Snapper.clearPointConstraintProvider(first)
        self.assertIsNone(draft.Snapper.pointConstraintProvider)


if __name__ == "__main__":
    unittest.main()
