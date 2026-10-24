"""Acceptance tests for the `ferrocad.stage` PyO3 surface (slice T5).

Runs against the extension module built from ``crates/ferrocad_stage_py`` — importable
as ``ferrocad_stage`` and (after ``install()``) as ``ferrocad.stage``.

Run directly::

    PYTHONPATH=python python3 tests/test_stage_py.py

The *same* file is executed by ``crates/ferrocad_stage_py/tests/stage_py.rs`` against an
embedded CPython interpreter (where ``install()`` has already registered the module), so
the embedded suite and the built-extension suite never drift.
"""

import array
import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

try:
    import ferrocad_stage as stage
except ImportError:  # the Rust harness registers it into sys.modules instead
    stage = sys.modules.get("ferrocad_stage")


needs_stage = unittest.skipIf(stage is None, "the ferrocad_stage extension is not built")


@needs_stage
class ModuleTest(unittest.TestCase):
    def test_install_registers_the_ferrocad_stage_namespace(self):
        import sys

        stage.install()
        self.assertIs(sys.modules["ferrocad.stage"], stage)
        self.assertIs(sys.modules["ferrocad_stage"], stage)


@needs_stage
class ShapeItemTest(unittest.TestCase):
    def test_ingests_a_contiguous_buffer(self):
        item = stage.ShapeItem(7)
        positions = array.array("f", [0, 0, 0, 1, 0, 0, 0, 1, 0])
        triangles = array.array("I", [0, 1, 2])
        item.set_shape(positions, None, triangles, None)
        self.assertEqual(
            item.positions, [(0.0, 0.0, 0.0), (1.0, 0.0, 0.0), (0.0, 1.0, 0.0)]
        )
        self.assertEqual(item.triangles, [0, 1, 2])
        self.assertEqual(item.entity_id, 7)
        self.assertEqual(item.draw_mode, stage.DrawMode.Shaded)

    def test_falls_back_to_python_sequences(self):
        item = stage.ShapeItem(1)
        item.set_shape([(0, 0, 0), (2, 0, 0), (0, 2, 0)], None, [0, 1, 2], [0, 1, 1, 2])
        self.assertEqual(
            item.positions, [(0.0, 0.0, 0.0), (2.0, 0.0, 0.0), (0.0, 2.0, 0.0)]
        )
        self.assertEqual(item.edges, [0, 1, 1, 2])

    def test_rejects_a_misaligned_buffer(self):
        item = stage.ShapeItem(1)
        with self.assertRaises(ValueError):
            item.set_shape(array.array("f", [0, 0, 0, 1]), None, None, None)

    def test_attributes_round_trip(self):
        item = stage.ShapeItem(3)
        item.draw_mode = stage.DrawMode.ShadedWithEdges
        item.surface_color = (1.0, 0.0, 0.0, 1.0)
        item.edge_color = (0.0, 0.0, 1.0, 1.0)
        item.edge_width = 2.5
        item.selectable = False
        self.assertEqual(item.draw_mode, stage.DrawMode.ShadedWithEdges)
        self.assertEqual(item.surface_color, (1.0, 0.0, 0.0, 1.0))
        self.assertEqual(item.edge_color, (0.0, 0.0, 1.0, 1.0))
        self.assertAlmostEqual(item.edge_width, 2.5)
        self.assertFalse(item.selectable)


@needs_stage
class StageTest(unittest.TestCase):
    def test_add_clear_and_emit(self):
        s = stage.Stage()
        self.assertTrue(s.is_empty())

        surface = stage.ShapeItem(1)
        surface.set_shape([(0, 0, 0), (1, 0, 0), (0, 1, 0)], None, [0, 1, 2], None)

        wire = stage.ShapeItem(2)
        wire.draw_mode = stage.DrawMode.Wireframe
        wire.set_shape([(0, 0, 0), (1, 0, 0), (0, 1, 0)], None, None, [0, 1, 1, 2, 2, 0])

        s.add(surface)
        s.add(wire)
        self.assertEqual(len(s), 2)
        self.assertEqual(s.entities(), [1, 2])
        # One surface packet + one wireframe packet.
        self.assertEqual(s.batch_count(), 2)

        s.clear()
        self.assertEqual(len(s), 0)
        self.assertEqual(s.batch_count(), 0)


@needs_stage
class ToolTest(unittest.TestCase):
    def test_a_python_tool_receives_events_and_adds_items(self):
        class Recorder(stage.Tool):
            def __init__(self):
                super().__init__()
                self.events = []

            def on_event(self, event):
                self.events.append((event.kind, event.position))
                marker = stage.ShapeItem(0)
                marker.draw_mode = stage.DrawMode.Wireframe
                marker.set_shape([(0, 0, 0), (1, 0, 0)], None, None, [0, 1])
                self.add_item(marker)
                return True

        tool = Recorder()
        self.assertEqual(tool.events, [])
        self.assertTrue(tool.on_event(stage.PointerEvent.move((10, 20))))
        self.assertEqual(tool.events, [(stage.PointerKind.Move, (10, 20))])
        self.assertEqual(len(tool.items), 1)
        tool.clear_items()
        self.assertEqual(len(tool.items), 0)


@needs_stage
class GizmoTest(unittest.TestCase):
    def test_a_drag_emits_drag_and_commit_events(self):
        camera = stage.Camera.perspective(5.0, size=(100, 100))
        gizmo = stage.TransformGizmo(7, stage.GizmoMode.Translate, (0.0, 0.0, 0.0))
        self.assertTrue(gizmo.update(camera))
        self.assertGreater(gizmo.scale, 0.0)

        gizmo.set_hovered(stage.GizmoHandle.AxisX)
        self.assertTrue(gizmo.begin_drag(stage.GizmoHandle.AxisX, camera, (50, 50)))
        self.assertTrue(gizmo.is_dragging)

        drag = gizmo.drag_to(camera, (75, 50))
        self.assertIsNotNone(drag)
        self.assertEqual(drag.target_id, 7)
        self.assertEqual(drag.active_handle, stage.GizmoHandle.AxisX)
        # A pure +X translation: row-major m[3] is tx, m[7]/m[11] are ty/tz.
        self.assertGreater(abs(drag.delta_transform[3]), 1e-6)
        self.assertAlmostEqual(drag.delta_transform[7], 0.0)
        self.assertAlmostEqual(drag.delta_transform[11], 0.0)

        commit = gizmo.end_drag()
        self.assertIsNotNone(commit)
        self.assertEqual(commit.target_id, 7)
        self.assertEqual(commit.total_transform, drag.total_transform)
        self.assertFalse(gizmo.is_dragging)

    def test_handle_event_dispatches_down_move_up(self):
        camera = stage.Camera.perspective(5.0, size=(100, 100))
        gizmo = stage.TransformGizmo(1, stage.GizmoMode.Translate)
        gizmo.set_hovered(stage.GizmoHandle.AxisY)

        down = stage.PointerEvent.down((50, 50), stage.MouseButton.Left)
        self.assertIsNone(gizmo.handle_event(camera, down))
        self.assertTrue(gizmo.is_dragging)

        emitted = gizmo.handle_event(camera, stage.PointerEvent.move((50, 75)))
        self.assertIsInstance(emitted, stage.GizmoDragEvent)

        up = stage.PointerEvent.up((50, 75), stage.MouseButton.Left)
        commit = gizmo.handle_event(camera, up)
        self.assertIsInstance(commit, stage.GizmoCommitEvent)
        self.assertFalse(gizmo.is_dragging)


@needs_stage
class SnapTest(unittest.TestCase):
    """The camera-free snap API over a workplane (slice Q4's foundation)."""

    def test_the_snap_surface_is_exported(self):
        for name in (
            "SnapKind",
            "Snap",
            "SnapCandidate",
            "SnapContext",
            "Workplane",
            "WorkplaneKind",
        ):
            self.assertTrue(hasattr(stage, name), name)
        # The six native kinds; no Draft-only modes.
        for name in ("Point", "Grid", "Vertex", "EdgeMid", "Centre", "Origin"):
            self.assertTrue(hasattr(stage.SnapKind, name), name)

    def test_the_world_workplane_round_trips(self):
        plane = stage.Workplane.world()
        self.assertEqual(plane.origin, (0.0, 0.0, 0.0))
        self.assertEqual(plane.u, (1.0, 0.0, 0.0))
        self.assertEqual(plane.v, (0.0, 1.0, 0.0))
        self.assertEqual(plane.n, (0.0, 0.0, 1.0))
        self.assertEqual(plane.point(3, 4), (3.0, 4.0, 0.0))
        self.assertEqual(plane.project(plane.point(3, 4)), (3.0, 4.0))
        # A point off the plane keeps its in-plane coordinates.
        self.assertEqual(plane.project((3.0, 4.0, 9.0)), (3.0, 4.0))

    def test_a_degenerate_workplane_raises(self):
        with self.assertRaises(ValueError):
            stage.Workplane((0, 0, 0), (0, 0, 0), (0, 1, 0))
        with self.assertRaises(ValueError):
            stage.Workplane(u=(1, 0, 0), v=(2, 0, 0))

    def test_workplane_kinds_are_readable(self):
        self.assertEqual(stage.WorkplaneKind.World.kind, "World")
        self.assertEqual(stage.WorkplaneKind.Datum("XY").kind, ("Datum", "XY"))
        self.assertEqual(
            stage.WorkplaneKind.Face(7, "Face6").kind, ("Face", 7, "Face6")
        )
        self.assertEqual(stage.Workplane.world().kind, stage.WorkplaneKind.World)

    def test_a_rotated_workplane_snaps_in_its_own_coordinates(self):
        s = 0.5**0.5
        plane = stage.Workplane((10, 0, 5), (s, s, 0), (-s, s, 0))
        self.assertAlmostEqual(plane.n[2], 1.0)
        self.assertAlmostEqual(plane.origin[0], 10.0)

        # The world point at local (2, 3), 4 above the plane (+n is +Z here).
        on_plane = plane.point(2, 3)
        off_plane = (on_plane[0], on_plane[1], on_plane[2] + 4.0)
        context = stage.SnapContext(plane, 1.0, 10.0)
        snap = context.snap_point(off_plane)

        self.assertEqual(snap.kind, stage.SnapKind.Grid)
        self.assertAlmostEqual(snap.local[0], 2.0, places=9)
        self.assertAlmostEqual(snap.local[1], 3.0, places=9)
        for actual, expected in zip(snap.point, plane.point(2, 3)):
            self.assertAlmostEqual(actual, expected, places=5)

    def test_a_feature_within_tolerance_wins_over_the_grid(self):
        context = stage.SnapContext.world(1.0)
        candidate = stage.SnapCandidate((2.4, 2.4, 0.0), stage.SnapKind.Vertex)

        inside = context.snap_point((2.1, 2.1, 0.0), [candidate], 0.5)
        self.assertEqual(inside.kind, stage.SnapKind.Vertex)
        self.assertEqual(inside.local, (2.4, 2.4))
        self.assertFalse(inside.major)
        for actual, expected in zip(inside.point, (2.4, 2.4, 0.0)):
            self.assertAlmostEqual(actual, expected, places=5)

        outside = context.snap_point((2.1, 2.1, 0.0), [candidate], 0.1)
        self.assertEqual(outside.kind, stage.SnapKind.Grid)
        self.assertEqual(outside.local, (2.0, 2.0))

    def test_the_major_flag_and_the_six_kinds(self):
        # `world(0.5)` has a decade-major step of 5.
        context = stage.SnapContext.world(0.5)
        self.assertEqual(context.minor, 0.5)
        self.assertEqual(context.major, 5.0)

        major = context.snap_point((5.2, 10.1, 0.0))
        self.assertEqual(major.kind, stage.SnapKind.Grid)
        self.assertEqual(major.local, (5.0, 10.0))
        self.assertTrue(major.major)

        minor = context.snap_point((1.2, 2.1, 0.0))
        self.assertFalse(minor.major)

        for name in ("Origin", "EdgeMid", "Centre"):
            kind = getattr(stage.SnapKind, name)
            candidate = stage.SnapCandidate((0.25, 0.25, 0.0), kind)
            snap = context.snap_point((0.25, 0.25, 0.0), [candidate], 0.5)
            self.assertEqual(snap.kind, kind)

    def test_a_non_positive_minor_returns_the_projected_point(self):
        plane = stage.Workplane((10, 0, 5), (0, 1, 0), (1, 0, 0))
        context = stage.SnapContext(plane, 0.0, 10.0)
        on_plane = plane.point(2.3, 3.7)
        snap = context.snap_point((on_plane[0], on_plane[1], on_plane[2] + 4.0))
        self.assertEqual(snap.kind, stage.SnapKind.Point)
        self.assertAlmostEqual(snap.local[0], 2.3, places=9)
        self.assertAlmostEqual(snap.local[1], 3.7, places=9)
        for actual, expected in zip(snap.point, on_plane):
            self.assertAlmostEqual(actual, expected, places=5)


if __name__ == "__main__":
    unittest.main()
