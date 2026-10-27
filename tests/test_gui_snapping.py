"""S4.4: SimpleCAD's own ``gui.snapping`` helpers over the real view facade.

The earlier S4 round measured the SimpleCAD unit stems with **stubbed** views, so a
green conformance line did not prove the adapter. These tests drive SimpleCAD's real
``gui.snapping`` code — ``ray``, ``camera_state``, ``pick`` and ``Snapper`` — against
the live ``FreeCADGui`` view whose camera and stage are published through
``ferrocad_stage``, exactly as the shell publishes them.

What this covers:

* ``snapping.ray`` builds ``coin.SbVec2f`` itself and compares the camera type with
  ``coin.SoPerspectiveCamera.getClassTypeId()`` — both are the names the shell's
  synthetic ``pivy.coin`` used to miss (its Rust surface is covered by
  ``crates/ferrocad_inventor_py/tests/coin_values.rs``);
* the screen projection, camera state and pick all read the published camera/stage;
* ``Snapper`` ranks a real ``Part.makeBox`` vertex, an on-edge point and a cylinder's
  circle centre through that projection.

Nothing here needs a window: the camera and stage are published the same way a
headless script publishes them.
"""

import importlib
import math
import os
import sys
import unittest
from types import SimpleNamespace
from unittest.mock import patch

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import FreeCAD
import FreeCADGui

try:
    import ferrocad_stage as stage
except ImportError:  # a build without the extension: every test skips
    stage = None

try:
    import Part
except ImportError:  # the Part workbench is a separate build
    Part = None

needs_stage = unittest.skipIf(stage is None, "the ferrocad_stage extension is not built")
needs_part = unittest.skipIf(Part is None, "the Part workbench is not built")


def _simplecad_snapping():
    """SimpleCAD's own ``gui.snapping`` (and ``core.geometry``), bound to real Coin.

    ``snapping`` does ``from pivy import coin`` at import time and builds
    ``coin.SbVec2f`` itself. Two other suites install a **stub** ``pivy.coin`` in
    ``sys.modules`` (and leave it there), so in a full run the module would bind the
    stub and the real snapping path could not run at all. The real extension is
    imported here, ``snapping`` is reloaded against it, and the process's own
    ``sys.modules`` entry is put back afterwards — the reloaded module keeps its
    reference, so this suite observes real Coin and the others keep their stub.
    """
    root = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "simplecad"))
    if not os.path.isdir(root):
        return None
    if root not in sys.path:
        sys.path.insert(0, root)
    saved = {
        name: sys.modules.pop(name)
        for name in ("pivy.coin", "pivy")
        if name in sys.modules
    }
    try:
        importlib.invalidate_caches()
        from pivy import coin as real_coin

        if not hasattr(real_coin, "SbVec2f") or not hasattr(
            real_coin, "SoPerspectiveCamera"
        ):
            return None
        from simplecad.core import geometry
        from simplecad.gui import snapping

        # A cached import may have bound the stub; re-execute the module body.
        snapping = importlib.reload(snapping)
    except Exception:  # noqa: BLE001 - an optional checkout, not a dependency
        return None
    finally:
        for name in ("pivy.coin", "pivy"):
            sys.modules.pop(name, None)
        sys.modules.update(saved)
    return SimpleNamespace(snapping=snapping, geometry=geometry)


SNAPPING = _simplecad_snapping()
needs_simplecad = unittest.skipIf(SNAPPING is None, "the SimpleCAD checkout is not present")


def _perspective(eye_z=100.0, size=(800, 600)):
    return stage.Camera.perspective(eye_z, size=size)


def _source(shape, name="Box", visible=True):
    """A snap target shaped like SimpleCAD's own test double."""
    return SimpleNamespace(
        Name=name,
        Shape=shape,
        TypeId="Part::Feature",
        InList=(),
        PropertiesList=[],
        Document=None,
        ViewObject=SimpleNamespace(
            Visibility=visible, Selectable=True, isVisible=lambda: visible
        ),
    )


@needs_stage
@needs_simplecad
class RayTest(unittest.TestCase):
    """``snapping.ray`` over the published camera."""

    def setUp(self):
        stage.clear_view()
        FreeCADGui._clear_documents()
        if FreeCAD.getDocument("SnapRay") is not None:
            FreeCAD.closeDocument("SnapRay")
        self.doc = FreeCAD.newDocument("SnapRay")
        self.view = FreeCADGui.getDocument("SnapRay").ActiveView
        stage.set_view_camera(_perspective())

    def tearDown(self):
        stage.clear_view()
        FreeCADGui._clear_documents()
        if FreeCAD.getDocument("SnapRay") is not None:
            FreeCAD.closeDocument("SnapRay")

    def test_a_perspective_ray_starts_at_the_near_plane_and_heads_into_the_scene(self):
        origin, direction = SNAPPING.snapping.ray(self.view, (400, 300))
        assert_all_close(self, origin, (0.0, 0.0, 99.9), places=3)
        # `ray` returns `end - origin` for a perspective camera.
        self.assertLess(direction.z, 0.0)
        self.assertAlmostEqual(direction.x, 0.0)
        self.assertAlmostEqual(direction.y, 0.0)
        self.assertAlmostEqual(direction.Length, 99.9, places=3)

    def test_an_orthographic_camera_returns_the_view_direction(self):
        stage.set_view_camera(stage.Camera.orthographic(10.0, size=(800, 600)))
        origin, direction = SNAPPING.snapping.ray(self.view, (400, 300))
        assert_all_close(self, direction, (0.0, 0.0, -1.0))
        self.assertAlmostEqual(origin.x, 0.0)
        self.assertAlmostEqual(origin.y, 0.0)

    def test_the_ray_follows_the_published_projection(self):
        before = SNAPPING.snapping.ray(self.view, (0, 0))[0]
        stage.set_view_camera(_perspective(eye_z=40.0))
        after = SNAPPING.snapping.ray(self.view, (0, 0))[0]
        self.assertNotAlmostEqual(before.z, after.z)

    def test_the_screen_point_agrees_with_the_ray_origin(self):
        screen = SNAPPING.snapping.screen(self.view, FreeCAD.Vector(0.0, 0.0, 0.0))
        self.assertEqual(screen, (400.0, 300.0))
        # The centre-line ray begins on the eye's axis, which projects to the centre.
        origin, _ = SNAPPING.snapping.ray(self.view, (400, 300))
        self.assertAlmostEqual(SNAPPING.snapping.screen(self.view, origin)[0], 400.0, places=6)

    def test_the_camera_state_is_stable_and_changes_with_the_camera(self):
        first = SNAPPING.snapping.camera_state(self.view)
        self.assertEqual(first, SNAPPING.snapping.camera_state(self.view))
        stage.set_view_camera(_perspective(eye_z=25.0))
        self.assertNotEqual(first, SNAPPING.snapping.camera_state(self.view))

    def test_a_type_id_naming_the_class_with_the_so_prefix_matches_too(self):
        """The shell's synthetic coin keeps the ``So`` prefix; real Coin drops it.

        ``snapping.ray`` decides perspective-vs-orthographic with
        ``camera.isOfType(coin.SoPerspectiveCamera.getClassTypeId())``, so the
        facade has to accept whichever spelling the installed Coin returns.
        """

        class _PrefixedId:
            def __init__(self, name):
                self._name = name

            def getName(self):
                return self._name

        camera = self.view.getCameraNode()
        self.assertTrue(camera.isOfType(_PrefixedId("SoPerspectiveCamera")))
        self.assertFalse(camera.isOfType(_PrefixedId("SoOrthographicCamera")))
        self.assertTrue(camera.isOfType(_PrefixedId("SoCamera")))


@needs_stage
@needs_part
@needs_simplecad
class SnapperTest(unittest.TestCase):
    """``Snapper`` ranks real kernel geometry through the facade's projection."""

    BOX = 20.0

    def setUp(self):
        stage.clear_view()
        FreeCADGui._clear_documents()
        if FreeCAD.getDocument("Snap") is not None:
            FreeCAD.closeDocument("Snap")
        self.doc = FreeCAD.newDocument("Snap")
        self.view = FreeCADGui.getDocument("Snap").ActiveView
        stage.set_view_camera(_perspective())
        self.shape = Part.makeBox(self.BOX, self.BOX, self.BOX)
        self.obj = _source(self.shape)
        self.document = SimpleNamespace(Name="Snap", Objects=[self.obj])
        self.snapper = SNAPPING.snapping.Snapper(self.document, self.view)
        self.plane = SNAPPING.geometry.Plane.named()
        patcher = patch("simplecad.gui.snapping.pick", return_value=None)
        patcher.start()
        self.addCleanup(patcher.stop)

    def tearDown(self):
        stage.clear_view()
        FreeCADGui._clear_documents()
        if FreeCAD.getDocument("Snap") is not None:
            FreeCAD.closeDocument("Snap")

    def pixel(self, point, dx=0, dy=0):
        sx, sy = self.view.getPointOnScreen(point)
        return (sx + dx, sy + dy)

    def test_a_vertex_snaps_exactly_through_the_facade_projection(self):
        corner = FreeCAD.Vector(self.BOX, self.BOX, self.BOX)
        snap, plane = self.snapper.snap_start(self.pixel(corner, 4, 2), self.plane)
        self.assertEqual(snap.label, "Endpoint")
        self.assertLess((snap.point - corner).Length, 1e-9)
        # The snap reports a real face plane through the point, not a frozen XY plane.
        self.assertLess(abs((corner - plane.origin).dot(plane.normal)), 1e-6)

    def test_a_pointer_between_a_vertex_and_the_midpoint_lands_on_the_edge(self):
        # An orthographic view has no foreshortening, so the face centres are far
        # outside their capture radius and the edge is the only candidate. (In the
        # perspective view they legitimately win: a face centre is 34 px away.)
        stage.set_view_camera(stage.Camera.orthographic(20.0, size=(800, 600)))
        snapper = SNAPPING.snapping.Snapper(self.document, self.view)
        corner = FreeCAD.Vector(self.BOX, 0.0, self.BOX)
        other = FreeCAD.Vector(self.BOX, self.BOX, self.BOX)
        target = corner + (other - corner) * 0.3
        pos = self.pixel(target, 1, 0)
        point = snapper.on_edge(pos)
        self.assertIsNotNone(point, "the pointer is on the edge")
        # The snap lies exactly on an edge of the shape, at the aimed-at parameter.
        # A top view looks straight down an edge and its opposite, so which of the
        # two depths wins is not fixed; the in-plane position is.
        self.assertLess(self.shape.distToShape(Part.Vertex(point))[0], 1e-6)
        self.assertAlmostEqual(point.x, self.BOX, places=6)
        self.assertAlmostEqual(point.y, 6.0, places=3)

        anchor = FreeCAD.Vector(target)
        snap, _plane = snapper.snap_line(self.pixel(target, 2, 0), self.plane, anchor, [])
        self.assertEqual(snap.label, "On edge")
        distance = self.shape.distToShape(Part.Vertex(snap.point))[0]
        self.assertLess(distance, 1e-6)

    def test_a_cylinders_top_circle_centre_is_found(self):
        cylinder = Part.makeCylinder(10.0, 20.0)
        top = max(cylinder.Edges, key=lambda edge: edge.BoundBox.ZMax)
        plane = SNAPPING.geometry.Plane.from_normal(
            FreeCAD.Vector(0, 0, 20), FreeCAD.Vector(0, 0, 1)
        )
        with patch(
            "simplecad.gui.snapping.pick",
            return_value=(self.obj, top, FreeCAD.Vector(10, 0, 20), "Edge1"),
        ):
            centre = self.snapper.center_hint((400, 300), plane)
        self.assertIsNotNone(centre)
        self.assertLess((centre - FreeCAD.Vector(0, 0, 20)).Length, 1e-6)


@needs_stage
@needs_part
@needs_simplecad
class PickThroughTheStageTest(unittest.TestCase):
    """``snapping.pick`` resolves the facade's ranked hit to a kernel face."""

    def setUp(self):
        stage.clear_view()
        FreeCADGui._clear_documents()
        if FreeCAD.getDocument("SnapPick") is not None:
            FreeCAD.closeDocument("SnapPick")
        self.doc = FreeCAD.newDocument("SnapPick")
        self.view = FreeCADGui.getDocument("SnapPick").ActiveView
        stage.set_view_camera(_perspective())
        self.shape = Part.makeBox(20.0, 20.0, 20.0)
        self.obj = _source(self.shape)

    def tearDown(self):
        stage.clear_view()
        FreeCADGui._clear_documents()
        if FreeCAD.getDocument("SnapPick") is not None:
            FreeCAD.closeDocument("SnapPick")

    def publish(self, name, corners, triangles, pixel):
        entity = 1
        positions = [(float(v.x), float(v.y), float(v.z)) for v in corners]
        # `tessellate` returns (i, j, k) tuples; `set_mesh` takes a flat index list.
        indices = [index for triangle in triangles for index in triangle]
        parts = [1] * len(positions)
        item = stage.ShapeItem(entity)
        item.set_mesh(positions, None, indices, [], parts)
        scene = stage.Stage()
        scene.add(item)
        stage.set_view_stage(scene, {entity: "Box"}, {entity: ["", name]})
        return SNAPPING.snapping.pick(
            self.view,
            SimpleNamespace(
                Name="SnapPick",
                getObject=lambda candidate: self.obj if candidate == "Box" else None,
            ),
            pixel,
            face_only=True,
        )

    def test_the_face_name_and_point_come_from_the_ranked_pick(self):
        face_index, top = next(
            (index, face)
            for index, face in enumerate(self.shape.Faces, 1)
            if SNAPPING.geometry.face_normal(face).z > 0.5
        )
        vertices, triangles = top.tessellate(0.1)
        pixel = self.view.getPointOnScreen(FreeCAD.Vector(10.0, 10.0, 20.0))
        result = self.publish(f"Face{face_index}", vertices, triangles, tuple(int(v) for v in pixel))
        self.assertIsNotNone(result, "the published face is picked")
        obj, face, point, name = result
        self.assertIs(obj, self.obj)
        self.assertEqual(name, f"Face{face_index}")
        self.assertAlmostEqual(point.z, 20.0, places=3)

    def test_empty_space_reports_none(self):
        vertices, triangles = self.shape.Faces[0].tessellate(0.1)
        self.assertIsNone(
            self.publish("Face1", vertices, triangles, tuple(self.view.getPointOnScreen(FreeCAD.Vector(0, 0, 0))))
        )


def assert_all_close(test, vector, expected, places=6):
    values = (vector.x, vector.y, vector.z)
    for value, wanted in zip(values, expected):
        test.assertAlmostEqual(value, wanted, places=places)


if __name__ == "__main__":
    unittest.main()
