"""S4.6: transient previews are ``ShapeItem``s, and the picker ignores them.

SimpleCAD draws its construction previews as imperative Coin subtrees. FerroCAD's
equivalent is a transient ``ferrocad_stage.ShapeItem``: ``FreeCADGui.Preview``
collects them and publishes the set, the shell draws it beside the document stage,
and the item is forced unpickable at the channel — so a preview never answers a
click and never appears in ``getObjectsInfo``.

The shell-side draw (merging the preview set into the stage it hands the viewport,
keyed on the generation) is covered where the shell exists; this file covers the
channel and the pick immunity, which are the parts a headless script sees.
"""

import os
import sys
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "python"))

import FreeCAD
import FreeCADGui

try:
    import ferrocad_stage as stage
except ImportError:  # a build without the extension: every test skips
    stage = None

needs_stage = unittest.skipIf(stage is None, "the ferrocad_stage extension is not built")


def _quad(z=-2.0, size=5.0):
    corners = [
        (-size, -size, z),
        (size, -size, z),
        (size, size, z),
        (-size, size, z),
    ]
    return corners, [0, 1, 2, 0, 2, 3]


@needs_stage
class PreviewChannelTest(unittest.TestCase):
    def setUp(self):
        stage.clear_view()
        stage.clear_previews()
        FreeCADGui._clear_documents()
        if FreeCAD.getDocument("Preview") is not None:
            FreeCAD.closeDocument("Preview")
        self.doc = FreeCAD.newDocument("Preview")
        self.doc.addObject("App::FeaturePython", "Body")
        self.view = FreeCADGui.getDocument("Preview").ActiveView
        stage.set_view_camera(stage.Camera.perspective(100.0, size=(800, 600)))

    def tearDown(self):
        stage.clear_previews()
        stage.clear_view()
        FreeCADGui._clear_documents()
        if FreeCAD.getDocument("Preview") is not None:
            FreeCAD.closeDocument("Preview")

    def test_update_publishes_the_set_and_bumps_the_generation(self):
        before = stage.preview_generation()
        preview = FreeCADGui.Preview(self.view)
        corners, triangles = _quad()
        preview.add_mesh(corners, triangles)
        self.assertEqual(len(preview), 1)
        generation = preview.update()
        self.assertGreater(generation, before)
        self.assertEqual(len(stage.preview_items()), 1)
        self.assertEqual(stage.preview_generation(), generation)

    def test_a_published_preview_is_never_selectable(self):
        preview = FreeCADGui.Preview(self.view)
        corners, triangles = _quad()
        preview.add_mesh(corners, triangles, shaded=False)
        preview.update()
        [item] = stage.preview_items()
        self.assertFalse(item.selectable, "the channel forces a preview unpickable")
        self.assertEqual(item.draw_mode, stage.DrawMode.Wireframe)
        self.assertEqual(item.edges, [])
        self.assertEqual(item.triangles, [0, 1, 2, 0, 2, 3])

    def test_add_accepts_a_ready_shape_item(self):
        item = stage.ShapeItem(4_000_000)
        corners, triangles = _quad()
        item.set_shape(corners, None, triangles, [])
        item.selectable = True
        preview = FreeCADGui.Preview(self.view)
        self.assertEqual(preview.add(item), 0)
        preview.update()
        [published] = stage.preview_items()
        self.assertFalse(published.selectable)

    def test_a_preview_can_be_removed_and_cleared(self):
        preview = FreeCADGui.Preview(self.view)
        corners, triangles = _quad()
        first = preview.add_mesh(corners, triangles)
        second = preview.add_mesh(corners, triangles, shaded=False)
        preview.update()
        self.assertEqual(len(stage.preview_items()), 2)
        self.assertTrue(preview.remove(first))
        self.assertFalse(preview.remove(first))
        preview.update()
        self.assertEqual(len(stage.preview_items()), 1)
        preview.clear()
        self.assertEqual(len(preview), 0)
        preview.update()
        self.assertEqual(stage.preview_items(), [])
        self.assertTrue(second >= 0)

    def test_a_preview_in_front_of_the_document_is_not_picked(self):
        # The document's face is behind the preview; a click at the centre must
        # still resolve to the document, because a preview is not a pick target.
        document = stage.ShapeItem(1)
        corners, triangles = _quad(z=0.0)
        document.set_mesh(corners, None, triangles, [], [1, 1, 1, 1])
        scene = stage.Stage()
        scene.add(document)
        stage.set_view_stage(scene, {1: "Body"}, {1: ["", "Face1"]})
        self.assertEqual(
            [(hit["Object"], hit["Component"]) for hit in self.view.getObjectsInfo((400, 300))],
            [("Body", "Face1")],
        )

        preview = FreeCADGui.Preview(self.view)
        corners, triangles = _quad(z=-5.0, size=20.0)
        preview.add_mesh(corners, triangles)
        preview.update()

        # A preview item is not in the published stage, so the ranked pick is
        # unchanged and the facade reports only the document.
        self.assertEqual(
            [(hit["Object"], hit["Component"]) for hit in self.view.getObjectsInfo((400, 300))],
            [("Body", "Face1")],
        )
        self.assertEqual(stage.pick_at(400, 300)[0].object, "Body")

    def test_the_colour_and_the_draw_mode_are_set(self):
        preview = FreeCADGui.Preview(self.view)
        corners, triangles = _quad()
        preview.add_mesh(corners, triangles, color=(1.0, 0.5, 0.25, 0.5), shaded=True)
        preview.update()
        [item] = stage.preview_items()
        self.assertEqual(item.surface_color, (1.0, 0.5, 0.25, 0.5))
        self.assertEqual(item.draw_mode, stage.DrawMode.ShadedWithEdges)

    def test_an_item_that_is_not_a_shape_item_is_rejected(self):
        preview = FreeCADGui.Preview(self.view)
        with self.assertRaises(TypeError):
            preview.add(object())


if __name__ == "__main__":
    unittest.main()
