"""Tests for the native Q4 Draft tool layer (``FerroCADGui.tools``).

The layer has three observable contracts:

* ``PlaneGui`` is backed by ``ferrocad.stage.Workplane`` and its
  ``get_placement()`` round-trips (the rotation columns are the workplane
  axes);
* ``Draft_Line``/``Draft_Wire`` run a Q4 point session: two wall-clock
  independent picks plus Enter create exactly one document object, Escape
  creates none, and ``draftToolBar.sourceCmd`` tracks the session;
* the commit is queued through ``QtCore.QTimer.singleShot(0, …)`` and executed
  through ``FreeCADGui.doCommand``.

``Part`` is not linked into the standalone ``ferrocad`` image used by this
suite (``tests/test_part.py`` skips for the same reason), so the tests install a
tiny ``Part`` stub with ``makePolygon`` for the duration of each creation test.
The tool still creates a real ``Part::Feature``; only the shape object is the
stub. When Part is available the same path uses ``Part.makePolygon`` unchanged.

Run::

    PYTHONPATH="$PWD/python" python3 -m pytest tests -q
"""

from __future__ import annotations

import contextlib
import json
import math
import os
import sys
import types
import unittest
from pathlib import Path
from unittest import mock

REPO = Path(__file__).resolve().parents[1]
PYTHON_DIR = REPO / "python"
if str(PYTHON_DIR) not in sys.path:
    sys.path.insert(0, str(PYTHON_DIR))

import FreeCAD
import FreeCADGui
import ferrocad_shell

try:
    import ferrocad_stage as stage
except ImportError:  # pragma: no cover - the harness may register it itself
    stage = sys.modules.get("ferrocad_stage")

needs_stage = unittest.skipIf(stage is None, "the ferrocad_stage extension is not built")

if stage is not None:
    import FerroCADGui.tools as tools
else:  # pragma: no cover - every test below is skipped
    tools = None

UPSTREAM = Path(os.environ.get("FERROCAD_UPSTREAM", REPO.parent / "freecad-upstream"))
UPSTREAM_LINES = UPSTREAM / "src" / "Mod" / "Draft" / "draftguitools" / "gui_lines.py"


# ---------------------------------------------------------------------------
# A tiny Part stub: only makePolygon is needed by Draft.make_wire/make_line
# ---------------------------------------------------------------------------


class _StubVertex:
    def __init__(self, point):
        self.Point = point


class _StubShape:
    def __init__(self, points):
        self.Vertexes = [_StubVertex(point) for point in points]
        self.Edges = []
        self._closed = bool(points) and points[0] == points[-1]

    def isClosed(self):
        return self._closed

    def copy(self):
        return _StubShape([vertex.Point for vertex in self.Vertexes])


@contextlib.contextmanager
def _part_module():
    """Yield a real ``Part`` when linked, else a minimal ``makePolygon`` stub."""
    try:
        import Part  # noqa: F401 - probe only

        yield sys.modules["Part"]
        return
    except Exception:  # noqa: BLE001 - the standalone image has no OCCT
        pass

    stub = types.ModuleType("Part")
    stub.makePolygon = lambda points: _StubShape(list(points))
    with mock.patch.dict(sys.modules, {"Part": stub}):
        yield stub


def _vector(value):
    return value if isinstance(value, FreeCAD.Vector) else FreeCAD.Vector(*value)


def _assert_vector_close(testcase, actual, expected, places=9):
    actual = _vector(actual)
    expected = _vector(expected)
    testcase.assertAlmostEqual(actual.x, expected.x, places=places)
    testcase.assertAlmostEqual(actual.y, expected.y, places=places)
    testcase.assertAlmostEqual(actual.z, expected.z, places=places)


# ---------------------------------------------------------------------------
# PlaneGui
# ---------------------------------------------------------------------------


@needs_stage
class PlaneGuiTests(unittest.TestCase):
    def setUp(self):
        self.saved = tools.get_working_plane().get_working_plane()
        self.addCleanup(tools.set_working_plane, self.saved)

    def test_default_is_the_world_plane(self):
        plane = tools.PlaneGui()

        workplane = plane.get_working_plane()
        self.assertIsInstance(workplane, stage.Workplane)
        self.assertEqual(workplane.origin, (0.0, 0.0, 0.0))
        self.assertEqual(workplane.u, (1.0, 0.0, 0.0))
        self.assertEqual(workplane.v, (0.0, 1.0, 0.0))
        self.assertEqual(workplane.n, (0.0, 0.0, 1.0))

    def test_get_and_set_working_plane(self):
        workplane = stage.Workplane(
            (1.0, 2.0, 3.0), (0.0, 1.0, 0.0), (-1.0, 0.0, 0.0)
        )
        plane = tools.PlaneGui()
        plane.set_working_plane(workplane)

        self.assertIs(plane.get_working_plane(), workplane)
        self.assertIs(plane.workplane, workplane)

    def test_world_placement_round_trips(self):
        plane = tools.PlaneGui()
        placement = plane.get_placement()

        self.assertIsInstance(placement, FreeCAD.Placement)
        _assert_vector_close(self, placement.Base, (0.0, 0.0, 0.0))
        _assert_vector_close(
            self, placement.Rotation.multVec(FreeCAD.Vector(1, 0, 0)), (1, 0, 0)
        )
        _assert_vector_close(
            self, placement.Rotation.multVec(FreeCAD.Vector(0, 1, 0)), (0, 1, 0)
        )
        _assert_vector_close(
            self, placement.Rotation.multVec(FreeCAD.Vector(0, 0, 1)), (0, 0, 1)
        )

    def test_rotated_placement_round_trips(self):
        root = 1.0 / math.sqrt(2.0)
        workplane = stage.Workplane(
            (10.0, 0.0, 5.0), (root, root, 0.0), (-root, root, 0.0)
        )
        plane = tools.PlaneGui(workplane=workplane)
        placement = plane.get_placement()

        _assert_vector_close(self, placement.Base, (10.0, 0.0, 5.0))

        u = placement.Rotation.multVec(FreeCAD.Vector(1, 0, 0))
        v = placement.Rotation.multVec(FreeCAD.Vector(0, 1, 0))
        n = placement.Rotation.multVec(FreeCAD.Vector(0, 0, 1))
        rebuilt = stage.Workplane(
            (placement.Base.x, placement.Base.y, placement.Base.z),
            (u.x, u.y, u.z),
            (v.x, v.y, v.z),
        )

        for actual, expected in (
            (rebuilt.origin, workplane.origin),
            (rebuilt.u, workplane.u),
            (rebuilt.v, workplane.v),
            (rebuilt.n, workplane.n),
        ):
            for got, want in zip(actual, expected):
                self.assertAlmostEqual(got, want, places=9)
        _assert_vector_close(self, n, workplane.n)

    def test_align_to_face_sets_origin_and_normal(self):
        plane = tools.PlaneGui()
        plane.align_to_face((10.0, 0.0, 5.0), (0.0, 0.0, 2.0))

        workplane = plane.get_working_plane()
        self.assertEqual(workplane.origin, (10.0, 0.0, 5.0))
        _assert_vector_close(self, workplane.n, (0.0, 0.0, 1.0))
        # u x v is the normal: the basis stays right-handed.
        _assert_vector_close(
            self,
            _vector(workplane.u).cross(_vector(workplane.v)),
            _vector(workplane.n),
        )

    def test_align_to_view_sets_the_normal(self):
        plane = tools.PlaneGui()
        plane.align_to_view((1.0, 0.0, 0.0))

        workplane = plane.get_working_plane()
        self.assertEqual(workplane.origin, (0.0, 0.0, 0.0))
        _assert_vector_close(self, workplane.n, (1.0, 0.0, 0.0))

    def test_reset_returns_to_the_world_plane(self):
        plane = tools.PlaneGui()
        plane.align_to_face((4.0, 4.0, 4.0), (1.0, 1.0, 1.0))
        plane.reset()

        workplane = plane.get_working_plane()
        self.assertEqual(workplane.origin, (0.0, 0.0, 0.0))
        self.assertEqual(workplane.u, (1.0, 0.0, 0.0))
        self.assertEqual(workplane.v, (0.0, 1.0, 0.0))
        self.assertEqual(workplane.n, (0.0, 0.0, 1.0))

    def test_update_grid_returns_the_view_request(self):
        plane = tools.PlaneGui()
        plane.grid_minor = 2.5
        plane.grid_major = 25.0

        request = plane._update_grid()

        self.assertEqual(set(request), {"view"})
        self.assertEqual(set(request["view"]), {"grid_minor", "grid_major"})
        self.assertAlmostEqual(request["view"]["grid_minor"], 2.5)
        self.assertAlmostEqual(request["view"]["grid_major"], 25.0)
        self.assertIs(plane.last_grid_request, request)

    def test_update_gui_records_the_label_without_a_qt_icon(self):
        plane = tools.PlaneGui()
        plane.label = "Front"

        self.assertIsNone(plane.icon)
        self.assertIsNone(plane.icon_path)
        self.assertIsNone(plane.update_gui())
        self.assertEqual(plane.status_label, "Front")
        self.assertEqual(plane.last_label, "Front")

    def test_set_to_view_returns_the_workplane_request(self):
        self.assertEqual(
            tools.PlaneGui().set_to_view(), {"view": {"workplane": "view"}}
        )

    def test_module_level_plane_accessors(self):
        root = 1.0 / math.sqrt(2.0)
        workplane = stage.Workplane(
            (10.0, 0.0, 5.0), (root, root, 0.0), (-root, root, 0.0)
        )

        plane = tools.set_working_plane(workplane)

        self.assertIs(tools.get_working_plane(), plane)
        self.assertIs(plane.get_working_plane(), workplane)
        _assert_vector_close(
            self, plane.get_placement().Base, (10.0, 0.0, 5.0)
        )

    def test_freecadgui_exposes_the_native_plane_class(self):
        self.assertIs(FreeCADGui.PlaneGui, tools.PlaneGui)


# ---------------------------------------------------------------------------
# Registration and diagnostics
# ---------------------------------------------------------------------------


@needs_stage
class RegistrationTests(unittest.TestCase):
    def setUp(self):
        tools.register_commands(replace=True)
        self.addCleanup(tools.register_commands, False)

    def test_line_and_wire_are_registered(self):
        self.assertIn("Draft_Line", FreeCADGui.listCommands())
        self.assertIn("Draft_Wire", FreeCADGui.listCommands())
        self.assertIsInstance(tools.get_command("Draft_Line"), tools.Line)
        self.assertIsInstance(tools.get_command("Draft_Wire"), tools.Wire)

    def test_commands_have_the_freecad_command_shape(self):
        for name in ("Draft_Line", "Draft_Wire"):
            command = tools.get_command(name)
            self.assertTrue(callable(command.Activated))
            self.assertTrue(callable(command.IsActive))
            resources = command.GetResources()
            self.assertIsInstance(resources, dict)
            self.assertIn("Pixmap", resources)
            self.assertIn("MenuText", resources)

    def test_the_native_path_is_documented(self):
        report = tools.upstream_report(UPSTREAM)
        self.assertEqual(report["path"], "native")
        self.assertTrue(report["blockers"])
        if UPSTREAM_LINES.is_file():
            self.assertTrue(report["present"])
        else:  # pragma: no cover - the checkout is present in this tree
            self.assertFalse(report["present"])

    @unittest.skipUnless(
        UPSTREAM_LINES.is_file(), "the upstream Draft checkout is unavailable"
    )
    def test_upstream_checkout_is_present_and_its_blockers_are_recorded(self):
        report = tools.upstream_report(UPSTREAM)

        self.assertTrue(report["present"])
        self.assertIn("ViewObject", " ".join(report["blockers"]))
        self.assertIn("Snapper.getPoint", " ".join(report["blockers"]))


# ---------------------------------------------------------------------------
# Line/Wire sessions
# ---------------------------------------------------------------------------


@needs_stage
class _SessionTestCase(unittest.TestCase):
    def setUp(self):
        tools.register_commands(replace=True)
        for tool in (tools.get_command("Draft_Line"), tools.get_command("Draft_Wire")):
            if tool is not None and tool.is_active:
                tool.escape()

        self.doc = FreeCAD.newDocument(self.id().replace(".", "_")[:40])
        FreeCAD.setActiveDocument(self.doc.Name)
        FreeCAD.activeDraftCommand = None
        self.line = tools.get_command("Draft_Line")
        self.wire = tools.get_command("Draft_Wire")
        self.addCleanup(self._cleanup)

    def _cleanup(self):
        for tool in (self.line, self.wire):
            if tool is not None and tool.is_active:
                tool.escape()
        FreeCAD.activeDraftCommand = None
        try:
            FreeCAD.closeDocument(self.doc.Name)
        except Exception:  # noqa: BLE001 - a failing test may have closed it
            pass
        tools.set_working_plane(stage.Workplane.world())

    def _activate(self, tool):
        self.assertIsNone(tool.Activated())
        self.assertTrue(tool.is_active)
        self.assertIs(FreeCADGui.draftToolBar.sourceCmd, tool)

    def _finish(self, tool):
        finish = getattr(FreeCADGui.draft, "finish", None)
        if callable(finish):
            return finish()
        return tool.finish()

    def _escape(self, tool):
        result = FreeCADGui.draft.escape()
        if tool.is_active:  # pragma: no cover - the Q4 escape must end the tool
            tool.escape()
        return result


class LineToolTests(_SessionTestCase):
    def test_two_picks_and_enter_create_one_line(self):
        first = FreeCAD.Vector(0.0, 0.0, 0.0)
        second = FreeCAD.Vector(5.0, 0.0, 0.0)

        with _part_module():
            self._activate(self.line)
            FreeCADGui.draft.pick((0.0, 0.0, 0.0))
            FreeCADGui.draft.pick((5.0, 0.0, 0.0))
            self._finish(self.line)

        self.assertEqual(len(self.doc.Objects), 1)
        shape = self.doc.Objects[0].Shape
        self.assertEqual(len(shape.Vertexes), 2)
        _assert_vector_close(self, shape.Vertexes[0].Point, first)
        _assert_vector_close(self, shape.Vertexes[-1].Point, second)
        self.assertIn("Draft.make_line", self.line.commit_text)

    def test_escape_after_one_point_leaves_the_document_unchanged(self):
        self._activate(self.line)
        FreeCADGui.draft.pick((1.0, 1.0, 0.0))
        self.assertEqual(len(self.line.node), 1)
        before = len(self.doc.Objects)

        self._escape(self.line)

        self.assertEqual(len(self.doc.Objects), before)
        self.assertEqual(len(self.line.node), 0)
        self.assertFalse(self.line.is_active)
        self.assertIsNone(FreeCADGui.draftToolBar.sourceCmd)
        self.assertIsNone(self.line.commit_text)

    def test_source_cmd_tracks_the_session(self):
        with _part_module():
            self._activate(self.line)
            self.assertIs(FreeCADGui.draftToolBar.sourceCmd, self.line)
            FreeCADGui.draft.pick((0.0, 0.0, 0.0))
            FreeCADGui.draft.pick((2.0, 0.0, 0.0))
            self._finish(self.line)

        self.assertIsNone(FreeCADGui.draftToolBar.sourceCmd)

    def test_each_pick_appends_and_shows_a_hint(self):
        self._activate(self.line)
        first_hint = self.line.last_hint
        self.assertIsNotNone(first_hint)

        FreeCADGui.draft.pick((0.0, 0.0, 0.0))

        self.assertEqual(len(self.line.node), 1)
        self.assertNotEqual(self.line.last_hint, first_hint)
        hint = FreeCADGui.HintManager.last
        self.assertIsNotNone(hint)
        self.assertEqual(hint.text, self.line.last_hint)

    def test_commit_is_queued_before_it_runs(self):
        seen = []
        original = FreeCADGui.doCommand

        def spy(text):
            seen.append(text)
            return original(text)

        with _part_module(), mock.patch.object(FreeCADGui, "doCommand", spy):
            self._activate(self.line)
            FreeCADGui.draft.pick((0.0, 0.0, 0.0))
            FreeCADGui.draft.pick((3.0, 0.0, 0.0))
            self._finish(self.line)

        self.assertEqual(len(seen), 1)
        self.assertIn("Draft.make_line", seen[0])
        events = self.line.events
        self.assertLess(events.index("queue"), events.index("commit"))
        self.assertGreater(
            events.index("queue"),
            max(index for index, event in enumerate(events) if event == "pick"),
        )
        # The PySide stub runs singleShot(0) immediately, so the commit is in
        # the same call stack but strictly after the second pick.
        self.assertEqual(len(self.doc.Objects), 1)


class WireToolTests(_SessionTestCase):
    def test_three_picks_and_enter_create_one_wire(self):
        points = [(0.0, 0.0, 0.0), (5.0, 0.0, 0.0), (5.0, 5.0, 0.0)]

        with _part_module():
            self._activate(self.wire)
            for point in points:
                FreeCADGui.draft.pick(point)
            self.assertEqual(len(self.wire.node), 3)
            self._finish(self.wire)

        self.assertEqual(len(self.doc.Objects), 1)
        shape = self.doc.Objects[0].Shape
        self.assertEqual(len(shape.Vertexes), 3)
        for vertex, expected in zip(shape.Vertexes, points):
            _assert_vector_close(self, vertex.Point, expected)
        self.assertIn("Draft.make_wire", self.wire.commit_text)

    def test_escape_after_two_points_creates_nothing(self):
        self._activate(self.wire)
        FreeCADGui.draft.pick((0.0, 0.0, 0.0))
        FreeCADGui.draft.pick((1.0, 0.0, 0.0))
        before = len(self.doc.Objects)

        self._escape(self.wire)

        self.assertEqual(len(self.doc.Objects), before)
        self.assertFalse(self.wire.is_active)
        self.assertIsNone(FreeCADGui.draftToolBar.sourceCmd)


# ---------------------------------------------------------------------------
# The shell bridge: a real viewport click reaches the Draft point session
# ---------------------------------------------------------------------------


@needs_stage
class DraftRoutingBridgeTests(_SessionTestCase):
    def test_the_command_requests_the_python_tool(self):
        payload = json.loads(ferrocad_shell.run_command("Draft_Line"))
        self.assertTrue(payload.get("ok"), payload)
        self.assertEqual(payload.get("tool"), "python:draft")
        self.assertTrue(self.line.is_active)

    def test_bridge_clicks_feed_the_session_and_finish_commits(self):
        payload = json.loads(ferrocad_shell.run_command("Draft_Line"))
        self.assertEqual(payload.get("tool"), "python:draft")

        bridge = ferrocad_shell.tool_bridge("draft")
        for point in ((0.0, 0.0, 0.0), (5.0, 0.0, 0.0)):
            event = stage.PointerEvent.down(
                (0, 0), stage.MouseButton.Left
            ).with_snap(point)
            self.assertTrue(bridge.on_event(event))

        status = bridge.status()
        self.assertTrue(status["active"])
        self.assertEqual(status["points"], 2)
        self.assertEqual(status["label"], "Line")

        with _part_module():
            self.assertTrue(ferrocad_shell.tool_command("draft", "finish"))

        self.assertEqual(len(self.doc.Objects), 1)
        self.assertFalse(self.line.is_active)
        self.assertIsNone(FreeCADGui.draftToolBar.sourceCmd)

    def test_escape_through_the_bridge_commits_nothing(self):
        json.loads(ferrocad_shell.run_command("Draft_Line"))
        bridge = ferrocad_shell.tool_bridge("draft")
        bridge.on_event(
            stage.PointerEvent.down((0, 0), stage.MouseButton.Left).with_snap(
                (1.0, 1.0, 0.0)
            )
        )
        before = len(self.doc.Objects)
        self.assertTrue(ferrocad_shell.tool_command("draft", "escape"))
        self.assertEqual(len(self.doc.Objects), before)
        self.assertFalse(self.line.is_active)


class HeadlessViewStyleTests(unittest.TestCase):
    """``SetStyle`` writes the line/point properties on the headless ViewObject."""

    def setUp(self):
        self.doc = FreeCAD.newDocument(self.id().replace(".", "_")[:40])
        self.obj = self.doc.addObject("App::FeatureTest", "Styled")
        self.addCleanup(self._cleanup)

    def _cleanup(self):
        try:
            FreeCAD.closeDocument(self.doc.Name)
        except Exception:  # noqa: BLE001 - a failing test may have closed it
            pass

    def test_the_style_properties_round_trip(self):
        view = self.obj.ViewObject
        view.LineColor = (1.0, 0.25, 0.0, 1.0)
        self.assertEqual(
            tuple(round(component, 3) for component in view.LineColor),
            (1.0, 0.25, 0.0, 1.0),
        )
        view.LineWidth = 4
        self.assertEqual(view.LineWidth, 4)
        view.PointColor = 0xFF0000FF
        self.assertEqual(
            tuple(round(component, 3) for component in view.PointColor),
            (1.0, 0.0, 0.0, 1.0),
        )
        view.PointSize = 6
        self.assertEqual(view.PointSize, 6)
        view.DrawStyle = "Dashed"
        self.assertEqual(view.DrawStyle, "Dashed")

    def test_an_unknown_draw_style_is_rejected(self):
        with self.assertRaises(ValueError):
            self.obj.ViewObject.DrawStyle = "Squiggly"


if __name__ == "__main__":
    unittest.main()
