"""Tests for the S4.1 view facade on ``FreeCADGui.ActiveDocument.ActiveView``.

The shell owns the camera and the displayed stage, and the facade cannot see an
``InventorViewport``; it reads what the host **published** into ``ferrocad_stage``
(``set_view_camera`` / ``set_view_stage``). These tests publish the same two values a
headless script or a test would, so the facade's projection, camera node and ranked
pick are exercised without a window:

* ``getPointOnScreen`` / ``getSize`` / ``getViewDirection`` follow the published camera
  (a second publication changes the answers — nothing is frozen);
* ``getCameraNode`` presents Coin's ``SoCamera`` vocabulary over that camera;
* ``getObjectsInfo`` returns the occlusion-ordered pick list with the sub-element name
  and the document object, and ``getObjectInfo`` is its first entry;
* ``addEventCallbackPivy`` / ``removeEventCallbackPivy`` and ``updateGui``.

See ``docs/project/research/simplecad-bringup.md`` §7 S4 and §5.1.
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import FreeCAD
import FreeCADGui

try:
    import ferrocad_stage as stage
except ImportError:  # a build without the extension: the camera tests skip
    stage = None

needs_stage = unittest.skipIf(stage is None, "the ferrocad_stage extension is not built")


def _simplecad_snapping():
    """SimpleCAD's own ``gui.snapping``, when the sibling checkout is present.

    The adapter's whole point is that SimpleCAD's controller runs close to unchanged;
    these tests call its real ``ray``/``camera_state``/``pick`` against the facade.
    """
    root = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "simplecad"))
    if not os.path.isdir(root):
        return None
    if root not in sys.path:
        sys.path.insert(0, root)
    try:
        from simplecad.gui import snapping
    except Exception:  # noqa: BLE001 - an optional checkout, not a dependency
        return None
    return snapping


SNAPPING = _simplecad_snapping()
needs_simplecad = unittest.skipIf(SNAPPING is None, "the SimpleCAD checkout is not present")


def _quad(entity, z, name="Face1", size=1.0):
    """A square in the plane ``z = z``, tagged as one sub-element."""
    corners = [
        (-size, -size, z),
        (size, -size, z),
        (size, size, z),
        (-size, size, z),
    ]
    item = stage.ShapeItem(entity)
    item.set_mesh(
        corners,
        None,
        [0, 1, 2, 0, 2, 3],
        [],
        [1, 1, 1, 1],
    )
    parts = {entity: ["", name]}
    return item, parts


def _camera(eye_z=100.0, size=(800, 600)):
    return stage.Camera.perspective(eye_z, size=size)


@needs_stage
class ViewCameraTest(unittest.TestCase):
    """The camera half of the facade."""

    def setUp(self):
        stage.clear_view()
        FreeCADGui._clear_documents()
        if FreeCAD.getDocument("Facade") is not None:
            FreeCAD.closeDocument("Facade")
        self.doc = FreeCAD.newDocument("Facade")
        self.doc.addObject("App::FeaturePython", "Body")
        self.view = FreeCADGui.getDocument("Facade").ActiveView
        self.camera = _camera()
        stage.set_view_camera(self.camera)

    def tearDown(self):
        stage.clear_view()
        FreeCADGui._clear_documents()
        if FreeCAD.getDocument("Facade") is not None:
            FreeCAD.closeDocument("Facade")

    def test_the_view_size_and_direction_come_from_the_published_camera(self):
        self.assertEqual(self.view.getSize(), (800, 600))
        direction = self.view.getViewDirection()
        self.assertAlmostEqual(direction.x, 0.0)
        self.assertAlmostEqual(direction.y, 0.0)
        self.assertAlmostEqual(direction.z, -1.0)

    def test_a_second_publication_moves_the_answers(self):
        self.assertEqual(self.view.getPointOnScreen((0.0, 0.0, 0.0)), (400.0, 300.0))
        stage.set_view_camera(stage.Camera.perspective(50.0, size=(400, 300)))
        self.assertEqual(self.view.getSize(), (400, 300))
        self.assertEqual(self.view.getPointOnScreen((0.0, 0.0, 0.0)), (200.0, 150.0))

    def test_a_point_behind_the_eye_has_no_screen_position(self):
        # The eye sits at z = 100 looking down -Z, so a point above it is behind the
        # camera and has no projection (reporting a mirrored pixel would be wrong).
        self.assertIsNone(self.view.getPointOnScreen((0.0, 0.0, 200.0)))

    def test_no_published_camera_is_reported_not_invented(self):
        stage.clear_view()
        self.assertIsNone(self.view.getCameraNode())
        self.assertEqual(self.view.getSize(), (0, 0))
        direction = self.view.getViewDirection()
        self.assertEqual((direction.x, direction.y, direction.z), (0.0, 0.0, -1.0))
        with self.assertRaises(RuntimeError):
            self.view.getPointOnScreen((0.0, 0.0, 0.0))

    def test_the_camera_node_answers_with_coin_vocabulary(self):
        node = self.view.getCameraNode()
        self.assertIsNotNone(node)
        self.assertAlmostEqual(node.position.getValue()[2], 100.0)
        self.assertEqual(node.getTypeId().getName(), "SoPerspectiveCamera")
        self.assertTrue(node.isOfType("SoCamera"))
        self.assertTrue(node.isOfType(node.getTypeId()))
        self.assertFalse(node.isOfType("SoOrthographicCamera"))
        self.assertIsNone(node.getField("noSuchField"))
        self.assertEqual(node.getField("aspectRatio").getValue(), 800 / 600)
        self.assertAlmostEqual(node.getField("heightAngle").getValue(), 45.0 * 3.141592653589793 / 180.0)
        with self.assertRaises(RuntimeError):
            node.getField("position").setValue((1.0, 2.0, 3.0))

    def test_the_view_volume_unprojects_a_line_of_sight(self):
        volume = self.view.getCameraNode().getViewVolume(800 / 600)
        near, far = volume.projectPointToLine((0.5, 0.5))
        self.assertEqual(near.getValue(), (0.0, 0.0, near.getValue()[2]))
        self.assertLess(far.getValue()[2], near.getValue()[2])
        # Looking down -Z, the centre line runs along z.
        self.assertAlmostEqual(near.x, 0.0)
        self.assertAlmostEqual(far.y, 0.0)

    def test_an_orthographic_camera_reports_its_own_type(self):
        stage.set_view_camera(stage.Camera.orthographic(10.0, size=(800, 600)))
        node = self.view.getCameraNode()
        self.assertEqual(node.getTypeId().getName(), "SoOrthographicCamera")
        self.assertTrue(node.isOfType("SoOrthographicCamera"))
        self.assertFalse(node.isOfType("SoPerspectiveCamera"))
        # A parallel camera's height is twice its vertical half-extent.
        self.assertAlmostEqual(node.getField("height").getValue(), 20.0)

    def test_the_simplecad_camera_state_changes_with_the_camera(self):
        """``snapping.camera_state`` caches screen positions under this tuple."""

        def state(view):
            camera = view.getCameraNode()
            values = [str(camera.getTypeId().getName())]
            for name in ("position", "orientation", "focalDistance", "height", "heightAngle", "aspectRatio"):
                field = camera.getField(name)
                if field is not None:
                    value = field.getValue()
                    values.append(tuple(value.getValue()) if hasattr(value, "getValue") else value)
            return tuple(values), tuple(view.getSize())

        before = state(self.view)
        stage.set_view_camera(_camera(eye_z=40.0))
        after = state(self.view)
        self.assertNotEqual(before, after)

    def test_update_gui_records_a_request(self):
        FreeCADGui._take_update_requests()
        before = self.view._updates
        self.view.updateGui()
        self.assertEqual(self.view._updates, before + 1)
        self.assertEqual(FreeCADGui._take_update_requests(), 1)
        self.assertEqual(FreeCADGui._take_update_requests(), 0)
        FreeCADGui.updateGui()
        self.assertEqual(FreeCADGui._take_update_requests(), 1)


@needs_stage
class RankedPickTest(unittest.TestCase):
    """``getObjectsInfo`` / ``getObjectInfo`` over the published stage."""

    def setUp(self):
        stage.clear_view()
        FreeCADGui._clear_documents()
        if FreeCAD.getDocument("Pick") is not None:
            FreeCAD.closeDocument("Pick")
        self.doc = FreeCAD.newDocument("Pick")
        self.doc.addObject("App::FeaturePython", "Near")
        self.doc.addObject("App::FeaturePython", "Far")
        self.doc.addObject("App::FeaturePython", "Decoy")
        self.view = FreeCADGui.getDocument("Pick").ActiveView
        stage.set_view_camera(_camera())

    def tearDown(self):
        stage.clear_view()
        FreeCADGui._clear_documents()
        if FreeCAD.getDocument("Pick") is not None:
            FreeCAD.closeDocument("Pick")

    def publish(self, *items):
        """Publish the items with an entity -> object mapping in entity order."""
        scene = stage.Stage()
        objects, parts = {}, {}
        for item, table in items:
            scene.add(item)
            objects[item.entity_id] = ("Near", "Far", "Decoy")[item.entity_id - 1]
            parts.update(table)
        stage.set_view_stage(scene, objects, parts)

    def test_the_pick_list_is_ordered_by_occlusion_and_names_the_part(self):
        near, near_parts = _quad(1, 0.0, "Face6")
        far, far_parts = _quad(2, -5.0, "Face3")
        self.publish((near, near_parts), (far, far_parts))

        hits = self.view.getObjectsInfo((400, 300))
        self.assertEqual(
            [(hit["Object"], hit["Component"]) for hit in hits],
            [("Near", "Face6"), ("Far", "Face3")],
        )
        self.assertTrue(all(hit["Document"] == "Pick" for hit in hits))
        self.assertAlmostEqual(hits[0]["z"], 0.0)
        self.assertAlmostEqual(hits[1]["z"], -5.0)
        self.assertEqual(hits[0]["x"], 0.0)
        self.assertEqual(hits[0]["y"], 0.0)

    def test_the_single_hit_is_the_first_of_the_ranked_list(self):
        near, near_parts = _quad(1, 0.0, "Face6")
        far, far_parts = _quad(2, -5.0, "Face3")
        self.publish((near, near_parts), (far, far_parts))
        self.assertEqual(self.view.getObjectInfo((400, 300)), self.view.getObjectsInfo((400, 300))[0])

    def test_an_unselectable_item_is_never_picked(self):
        decoy, decoy_parts = _quad(3, 2.0, "Face1")
        decoy.selectable = False
        near, near_parts = _quad(1, 0.0, "Face6")
        self.publish((decoy, decoy_parts), (near, near_parts))
        hits = self.view.getObjectsInfo((400, 300))
        self.assertEqual([hit["Object"] for hit in hits], ["Near"])

    def test_empty_space_has_no_hits(self):
        near, near_parts = _quad(1, 0.0, "Face6", size=0.1)
        self.publish((near, near_parts))
        self.assertEqual(self.view.getObjectsInfo((0, 0)), [])
        self.assertIsNone(self.view.getObjectInfo((0, 0)))

    def test_a_pick_reads_the_screen_position_the_projection_reports(self):
        near, near_parts = _quad(1, 0.0, "Face6", size=10.0)
        self.publish((near, near_parts))
        pixel = self.view.getPointOnScreen((2.0, -3.0, 0.0))
        hit = self.view.getObjectInfo((int(round(pixel[0])), int(round(pixel[1]))))
        self.assertIsNotNone(hit)
        self.assertEqual(hit["Component"], "Face6")
        self.assertAlmostEqual(hit["x"], 2.0, delta=0.2)
        self.assertAlmostEqual(hit["y"], -3.0, delta=0.2)
        self.assertAlmostEqual(hit["z"], 0.0, places=6)

    def test_without_a_published_stage_there_is_nothing_to_pick(self):
        stage.clear_view()
        stage.set_view_camera(_camera())
        self.assertEqual(self.view.getObjectsInfo((400, 300)), [])


@needs_simplecad
class SimpleCadAdapterTest(unittest.TestCase):
    """SimpleCAD's ``gui.snapping`` helpers read the facade unchanged."""

    def setUp(self):
        stage.clear_view()
        FreeCADGui._clear_documents()
        if FreeCAD.getDocument("Simple") is not None:
            FreeCAD.closeDocument("Simple")
        self.doc = FreeCAD.newDocument("Simple")
        self.obj = self.doc.addObject("App::FeaturePython", "Body")
        self.view = FreeCADGui.getDocument("Simple").ActiveView
        stage.set_view_camera(_camera())

    def tearDown(self):
        stage.clear_view()
        FreeCADGui._clear_documents()
        if FreeCAD.getDocument("Simple") is not None:
            FreeCAD.closeDocument("Simple")

    def publish_whole_object(self):
        """A quad whose only sub-element is the whole object (``Component == ""``)."""
        item = stage.ShapeItem(5)
        item.set_shape([(-5, -5, 0), (5, -5, 0), (5, 5, 0), (-5, 5, 0)], None, [0, 1, 2, 0, 2, 3], [])
        scene = stage.Stage()
        scene.add(item)
        stage.set_view_stage(scene, {5: "Body"}, None)

    def test_simplecads_ray_reads_the_camera_node(self):
        coin = getattr(SNAPPING, "coin", None)
        if not hasattr(coin, "SbVec2f"):
            # SimpleCAD builds an `SbVec2f` itself. FerroCAD's synthetic `pivy.coin`
            # (like the stub another suite installs in this process) has no `SbVec2f`;
            # the facade's own volume accepts a plain pair, which is covered above.
            self.skipTest("the pivy.coin in this process has no SbVec2f")
        origin, direction = SNAPPING.ray(self.view, (400, 300))
        self.assertAlmostEqual(origin.x, 0.0)
        self.assertAlmostEqual(origin.y, 0.0)
        self.assertAlmostEqual(origin.z, 99.9)
        # A perspective camera returns `end - origin`, which is along -Z here.
        self.assertLess(direction.z, 0.0)
        self.assertAlmostEqual(direction.x, 0.0)
        self.assertAlmostEqual(direction.y, 0.0)

    def test_simplecads_camera_state_is_stable_until_the_camera_moves(self):
        before = SNAPPING.camera_state(self.view)
        self.assertEqual(before, SNAPPING.camera_state(self.view))
        stage.set_view_camera(_camera(eye_z=30.0))
        self.assertNotEqual(before, SNAPPING.camera_state(self.view))

    def test_simplecads_pick_resolves_its_hit_through_the_facade(self):
        self.publish_whole_object()
        from types import SimpleNamespace

        visible = SimpleNamespace(Visibility=True)
        body = SimpleNamespace(
            Name="Body",
            PropertiesList=[],
            Shape=SimpleNamespace(getElement=lambda name: name),
            ViewObject=visible,
        )
        doc = SimpleNamespace(Name="Simple", getObject=lambda name: body if name == "Body" else None)
        obj, shape, point, name = SNAPPING.pick(self.view, doc, (400, 300))
        self.assertIs(obj, body)
        self.assertEqual(name, "")
        self.assertAlmostEqual(point.x, 0.0, places=3)
        self.assertAlmostEqual(point.z, 0.0, places=3)

    def test_simplecads_pick_prefers_an_sc_support_profile(self):
        self.publish_whole_object()
        from types import SimpleNamespace

        visible = SimpleNamespace(Visibility=True)
        host = SimpleNamespace(Name="Body", PropertiesList=[], Shape=object(), ViewObject=visible)
        profile = SimpleNamespace(Name="Profile", PropertiesList=["SC_Support"], Shape=object(), ViewObject=visible)
        doc = SimpleNamespace(
            Name="Simple",
            getObject=lambda name: {"Body": host, "Profile": profile}.get(name),
        )
        scene = stage.Stage()
        for entity, name in ((5, "Body"), (6, "Profile")):
            item = stage.ShapeItem(entity)
            item.set_shape(
                [(-5, -5, 0), (5, -5, 0), (5, 5, 0), (-5, 5, 0)],
                None,
                [0, 1, 2, 0, 2, 3],
                [],
            )
            scene.add(item)
        # The host is nearer than the profile: the document's own priority must win,
        # which is exactly the sort SimpleCAD applies to the ranked list.
        stage.set_view_stage(scene, {5: "Body", 6: "Profile"}, None)
        obj, _shape, _point, _name = SNAPPING.pick(self.view, doc, (400, 300))
        self.assertIs(obj, profile)


class PivyEventCallbackTest(unittest.TestCase):
    """The Pivy spelling of the view's event callbacks."""

    def setUp(self):
        FreeCADGui._clear_documents()
        if FreeCAD.getDocument("Events") is not None:
            FreeCAD.closeDocument("Events")
        self.doc = FreeCAD.newDocument("Events")
        self.view = FreeCADGui.getDocument("Events").ActiveView
        self.event_type = self._event_type()

    def tearDown(self):
        for registration in list(self.view._pivy):
            registration.deactivate()
        self.view._pivy = []
        FreeCADGui._clear_documents()
        if FreeCAD.getDocument("Events") is not None:
            FreeCAD.closeDocument("Events")

    @staticmethod
    def _event_type():
        """A Coin mouse event type id when Coin is available, else its shape."""
        try:
            from pivy import coin

            return coin.SoMouseButtonEvent.getClassTypeId()
        except Exception:  # noqa: BLE001 - the facade's own stub is enough
            class _Name:
                @staticmethod
                def getName():
                    return "SoMouseButtonEvent"

            class _Type:
                @staticmethod
                def getClassTypeId():
                    return _Name()

            return _Type

    def test_the_callback_receives_a_handle_and_can_be_removed(self):
        seen = []
        handle = self.view.addEventCallbackPivy(self.event_type, seen.append)
        self.assertEqual(len(self.view._pivy), 1)
        self.view._pivy[0].dispatch()
        self.assertEqual(seen, [handle])

        handle.setHandled()
        self.assertTrue(handle.handled)
        self.assertIsNone(handle.getAction())
        self.assertIsNone(handle.getEvent())

        self.assertTrue(self.view.removeEventCallbackPivy(self.event_type, handle))
        self.assertFalse(self.view.removeEventCallbackPivy(self.event_type, handle))
        self.assertEqual(self.view._pivy, [])
        self.assertEqual(len(seen), 1, "a removed registration forwards nothing")

    def test_the_user_callback_itself_can_be_removed(self):
        calls = []
        self.view.addEventCallbackPivy(self.event_type, calls.append)
        self.assertTrue(self.view.removeEventCallbackPivy(self.event_type, calls.append))

    def test_a_non_callable_is_rejected(self):
        with self.assertRaises(TypeError):
            self.view.addEventCallbackPivy(self.event_type, None)

    def test_the_string_registry_still_works_alongside(self):
        calls = []
        self.view.addEventCallback("mouse", calls.append)
        self.assertTrue(self.view.removeEventCallback("mouse", calls.append))
        self.assertFalse(self.view.removeEventCallback("mouse", calls.append))


if __name__ == "__main__":
    unittest.main()
