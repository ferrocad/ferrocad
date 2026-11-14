"""The Sculpt bundle end to end: cage -> Catmull-Clark -> B-rep -> Body.

`Sculpt` is the workbench the ideation note's "bundle" tier asks for: the
document model a subdivision modeller needs, with the interaction and the body
rules owned by FerroCAD rather than by an add-on. Three pieces are under test.

**The cage** — a ``Mesh::FeaturePython`` holding ``CagePoints``/``CageFaces``
with a Catmull-Clark preview in ``Mesh``. Editing a point re-runs the proxy.

**The skin** — a ``Part::FeaturePython`` linking back to the cage, subdividing
it into one bicubic patch per quad, sewing the shell and storing a solid.

**The nesting** — a ``PartDesign::Body`` takes the cage as a *support* (it is
not the body's shape), the skin as its ``BaseFeature``, and a PartDesign feature
as the ``Tip``. The role table is
``docs/developer/workbenches/sculpt.md``; the body rules are
``docs/developer/workbenches/part-shape-edits.md``.

The geometry contract this module pins: the refinement follows the standard
Catmull-Clark rules *including the boundary rules* (an open cage keeps its rim),
the skin is a single closed valid solid, and more levels converge monotonically
on the limit surface while the volume falls from the cage's box.
"""

import os
import tempfile
import unittest

import FreeCAD
import Mesh
import Part
import Sculpt

V = FreeCAD.Vector


def _edges_used_once(points, quads):
    """The edges of the quad mesh that are not shared by exactly two faces."""
    uses = {}
    for a, b, c, d in quads:
        for edge in ((a, b), (b, c), (c, d), (d, a)):
            key = tuple(sorted(edge))
            uses[key] = uses.get(key, 0) + 1
    return [key for key, count in uses.items() if count != 2]


def _mesh_of(points, quads):
    return Mesh.Mesh((list(points), Sculpt.triangles(quads)))


class SculptCase(unittest.TestCase):
    def setUp(self):
        self.doc = FreeCAD.newDocument(self.id().replace(".", "_"))

    def tearDown(self):
        if FreeCAD.getDocument(self.doc.Name) is not None:
            FreeCAD.closeDocument(self.doc.Name)


class SubdivisionTests(unittest.TestCase):
    def test_a_single_quad_refines_into_four_and_keeps_its_rim(self):
        points = [V(0, 0, 0), V(10, 0, 0), V(10, 10, 0), V(0, 10, 0)]
        refined, quads = Sculpt.catmull_clark(points, [(0, 1, 2, 3)])

        # Four corners, one face point, four edge points.
        self.assertEqual(len(quads), 4)
        self.assertEqual(len(refined), 9)

        # The corners are boundary corners and stay put, and the rim is still
        # the original square: an open cage is not pulled inward.
        for corner in points:
            self.assertIn(corner, refined, "a boundary corner is not moved")
        rim = _edges_used_once(refined, quads)
        self.assertEqual(len(rim), 8, "each side is split by its new edge point")
        for a, b in rim:
            for index in (a, b):
                point = refined[index]
                self.assertEqual(point.z, 0.0)
                self.assertTrue(
                    point.x in (0.0, 10.0) or point.y in (0.0, 10.0),
                    "%s is off the original square" % (point,),
                )

        # One refinement reproduces the quad's area exactly (the face point is
        # the centroid and the edge points are midpoints).
        self.assertAlmostEqual(_mesh_of(refined, quads).Area, 100.0, places=9)
        self.assertEqual(_mesh_of(refined, quads).CountFacets, 8)

    def test_the_boundary_rules_keep_the_rim_and_smooth_the_crease(self):
        # A bent 2x1 strip: the middle column is raised, so the shared edge is a
        # crease and the four outer corners are the only fixed points.
        points = [
            V(0, 0, 0), V(10, 0, 5), V(20, 0, 0),
            V(0, 10, 0), V(10, 10, 5), V(20, 10, 0),
        ]
        quads = [(0, 1, 4, 3), (1, 2, 5, 4)]
        refined, _faces = Sculpt.catmull_clark(points, quads)
        for corner in (0, 2, 3, 5):
            self.assertIn(points[corner], refined, "a boundary corner is not moved")
        # A vertex on exactly two boundary edges follows the (1,6,1)/8 curve
        # rule, so the crease's apex drops toward its neighbours' plane.
        self.assertAlmostEqual(refined[1].x, 10.0, places=9)
        self.assertAlmostEqual(refined[1].z, 3.75, places=9)
        # A boundary edge's point is its midpoint.
        self.assertIn(V(5, 0, 2.5), refined)

    def test_a_box_cage_refines_closed_and_quad_for_quad(self):
        points, faces = Sculpt.box_cage(40, 30, 20)
        quads = Sculpt.cage_quads(faces)
        self.assertEqual(len(quads), 6)
        self.assertEqual(_edges_used_once(points, quads), [], "the cage is closed")

        vertices, faces_count = len(points), len(quads)
        for _level in range(3):
            points, quads = Sculpt.catmull_clark(points, quads)
            self.assertEqual(len(quads), faces_count * 4, "one quad per corner")
            self.assertEqual(_edges_used_once(points, quads), [], "still closed")
            # A closed quad mesh has two edges per face, so a refinement adds a
            # face point and two edge points per face.
            self.assertEqual(len(points), vertices + 3 * faces_count)
            vertices, faces_count = len(points), len(quads)

    def test_a_face_list_must_hold_four_indices(self):
        with self.assertRaises(ValueError):
            Sculpt.cage_quads([0, 1, 2])

    def test_a_face_cannot_reference_a_missing_point(self):
        with self.assertRaises(ValueError):
            Sculpt.catmull_clark([V(0, 0, 0), V(1, 0, 0), V(0, 1, 0)], [(0, 1, 7, 2)])


class CageObjectTests(SculptCase):
    def test_the_cage_previews_the_subdivision(self):
        points, faces = Sculpt.box_cage(40, 30, 20)
        cage = Sculpt.makeCage(self.doc, "Cage", points, faces, levels=1)
        self.doc.recompute()

        self.assertEqual(int(cage.Levels), 1)
        self.assertEqual(cage.Mesh.CountFacets, 6 * 4 * 2, "24 quads, two triangles each")
        self.assertGreater(cage.Mesh.Volume, 0.0, "the preview is outward wound")

    def test_an_edit_reruns_the_preview(self):
        points, faces = Sculpt.box_cage(20, 20, 20)
        cage = Sculpt.makeCage(self.doc, "Cage", points, faces, levels=1)
        self.doc.recompute()
        before = cage.Mesh.Volume

        moved = list(cage.CagePoints)
        moved[6] = V(40, 20, 20)
        cage.CagePoints = moved
        self.doc.recompute()
        self.assertGreater(cage.Mesh.Volume, before, "the dragged corner reaches the mesh")

    def test_the_preview_follows_the_placement(self):
        points, faces = Sculpt.box_cage(20, 20, 20)
        cage = Sculpt.makeCage(self.doc, "Cage", points, faces, levels=0)
        self.doc.recompute()
        cage.Placement = FreeCAD.Placement(V(100, 0, 0), FreeCAD.Rotation())
        box = cage.Mesh.BoundBox
        self.assertAlmostEqual(box.XMin, 100.0, places=6)


class SkinTests(SculptCase):
    def make_box(self, levels=1):
        points, faces = Sculpt.box_cage(40, 30, 20)
        cage = Sculpt.makeCage(self.doc, "Cage", points, faces, levels=levels)
        skin = Sculpt.makeSkin(self.doc, "Skin", cage)
        self.doc.recompute()
        return cage, skin

    def test_the_skin_sews_one_closed_valid_solid(self):
        _cage, skin = self.make_box(levels=1)
        shape = skin.Shape
        self.assertTrue(shape.isValid(), skin.getStatusString())
        self.assertTrue(shape.isClosed())
        self.assertEqual(len(shape.Solids), 1)
        self.assertEqual(len(shape.Faces), 24, "one patch per refined quad")
        # The patch skin stays inside the cage and outside nothing: its volume
        # is below the 40x30x20 box and above the level-1 preview's own volume,
        # because a bilinear patch through the quad corners bulges slightly.
        self.assertLess(shape.Volume, 40 * 30 * 20)
        self.assertGreater(shape.Volume, 8000.0)

    def test_more_levels_converge_on_the_limit_surface(self):
        points, faces = Sculpt.box_cage(40, 30, 20)
        cage = Sculpt.makeCage(self.doc, "Cage", points, faces, levels=0)
        skin = Sculpt.makeSkin(self.doc, "Skin", cage)
        volumes = []
        for levels in (1, 2, 3):
            cage.Levels = levels
            self.doc.recompute()
            volumes.append(skin.Shape.Volume)
            self.assertTrue(skin.Shape.isValid(), "level %d is valid" % levels)
        # The limit surface is approached from above the faceted mesh and below
        # the cage; each refinement moves the volume by less than the one before.
        steps = [abs(b - a) for a, b in zip(volumes, volumes[1:])]
        self.assertTrue(volumes[0] > volumes[1] > volumes[2], volumes)
        self.assertLess(steps[1], steps[0], "the second step is smaller")
        self.assertLess(steps[1] / volumes[2], 0.06, "level 3 is close to the limit")

    def test_the_skin_follows_the_cage_placement_and_edits(self):
        cage, skin = self.make_box(levels=1)
        volume = skin.Shape.Volume

        cage.Placement = FreeCAD.Placement(V(100, 0, 0), FreeCAD.Rotation())
        self.doc.recompute()
        self.assertAlmostEqual(skin.Shape.BoundBox.XMin, 100.0, places=4)
        self.assertAlmostEqual(skin.Shape.Volume, volume, places=3)

        moved = list(cage.CagePoints)
        moved[6] = V(60, 30, 20)
        cage.CagePoints = moved
        self.doc.recompute()
        self.assertGreater(skin.Shape.Volume, volume, "a cage drag reaches the solid")

    def test_the_skin_can_have_its_own_resolution(self):
        points, faces = Sculpt.box_cage(20, 20, 20)
        cage = Sculpt.makeCage(self.doc, "Cage", points, faces, levels=1)
        skin = Sculpt.makeSkin(self.doc, "Skin", cage, levels=2)
        self.doc.recompute()
        # 96 patches: the skin is finer than the cage's own level-1 preview.
        self.assertEqual(len(skin.Shape.Faces), 96)
        self.assertEqual(cage.Mesh.CountFacets, 48)

    def test_an_open_cage_is_reported_not_silently_solid(self):
        cage = Sculpt.makeCage(
            self.doc,
            "Open",
            [V(0, 0, 0), V(10, 0, 0), V(10, 10, 0), V(0, 10, 0)],
            [0, 1, 2, 3],
            levels=1,
        )
        skin = Sculpt.makeSkin(self.doc, "OpenSkin", cage)
        self.doc.recompute()
        self.assertFalse(skin.isValid())
        self.assertIn("not closed", skin.getStatusString())
        self.assertTrue(cage.Mesh.CountFacets > 0, "the preview still works")

    def test_a_skin_needs_a_cage(self):
        with self.assertRaises(ValueError):
            Sculpt.makeSkin(self.doc, "Skin", None)


class BodyTests(SculptCase):
    def setUp(self):
        super().setUp()
        self.points, self.faces = Sculpt.box_cage(40, 30, 20)
        self.cage = Sculpt.makeCage(self.doc, "Cage", self.points, self.faces, levels=2)
        self.skin = Sculpt.makeSkin(self.doc, "Skin", self.cage)
        self.body = self.doc.addObject("PartDesign::Body", "Body")

    def test_the_cage_supports_and_the_skin_is_the_base(self):
        roles = Sculpt.nest(self.body, self.cage, self.skin)
        self.assertEqual(roles, ["support", "base"])
        self.doc.recompute()

        self.assertEqual([member.Name for member in self.body.Group], ["Cage", "Skin"])
        self.assertIs(self.body.BaseFeature, self.skin)
        self.assertEqual(Sculpt.role_of(self.body, self.skin), "base")
        self.assertEqual(Sculpt.role_of(self.body, self.cage), "support")

    def test_the_body_shape_is_the_skin(self):
        Sculpt.nest(self.body, self.cage, self.skin)
        self.doc.recompute()
        self.assertEqual(self.body.Shape.ShapeType, "Solid")
        self.assertTrue(self.body.Shape.isValid())
        self.assertAlmostEqual(
            self.body.Shape.Volume, self.skin.Shape.Volume, places=6
        )

    def test_the_body_placement_moves_the_sculpted_solid(self):
        Sculpt.nest(self.body, self.cage, self.skin)
        self.doc.recompute()
        before = self.body.Shape.BoundBox.ZMin
        self.body.Placement = FreeCAD.Placement(V(0, 0, 50), FreeCAD.Rotation())
        self.doc.recompute()
        self.assertAlmostEqual(self.body.Shape.BoundBox.ZMin, before + 50.0, places=4)

    def test_a_second_base_is_refused(self):
        part = self.doc.addObject("Part::Feature", "Plain")
        part.Shape = Part.makeBox(1, 1, 1)
        Sculpt.nest(self.body, self.skin)
        with self.assertRaisesRegex(ValueError, "base feature"):
            self.body.addObject(part)
        with self.assertRaisesRegex(ValueError, "base feature"):
            Sculpt.nest(self.body, self.doc.addObject("Part::FeaturePython", "Other"))

    def test_a_mesh_that_is_not_a_cage_is_refused(self):
        with self.assertRaisesRegex(ValueError, "not allowed"):
            Sculpt.nest(self.body, self.doc.addObject("App::FeaturePython", "Proxy"))

    def test_a_partdesign_feature_takes_the_tip(self):
        # The role table, on its own body: a base and then a feature. The box is
        # standalone geometry (the dress-up features are the ones that chain);
        # what is under test is that the *role* moves, not the shape.
        body = self.doc.addObject("PartDesign::Body", "FeatureBody")
        base = self.doc.addObject("Part::FeaturePython", "Base")
        feature = self.doc.addObject("PartDesign::AdditiveBox", "Pad")
        self.assertEqual(Sculpt.nest(body, base), ["base"])
        self.assertEqual(Sculpt.nest(body, feature), ["tip"])
        self.doc.recompute()
        self.assertEqual(body.Tip.Name, feature.Name)
        self.assertEqual(body.BaseFeature.Name, base.Name)
        self.assertEqual(Sculpt.role_of(body, feature), "tip")
        self.assertEqual(Sculpt.role_of(body, base), "base")


class PersistenceTests(SculptCase):
    def build(self, levels=1):
        points, faces = Sculpt.box_cage(20, 20, 20)
        cage = Sculpt.makeCage(self.doc, "Cage", points, faces, levels=levels)
        skin = Sculpt.makeSkin(self.doc, "Skin", cage)
        body = self.doc.addObject("PartDesign::Body", "Body")
        Sculpt.nest(body, cage, skin)
        self.doc.recompute()
        return cage, skin, body

    def test_a_native_round_trip_restores_both_proxies(self):
        _cage, skin, body = self.build(levels=1)
        volume = body.Shape.Volume
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "sculpt.ferrocad")
            self.doc.saveAs(path)
            FreeCAD.closeDocument(self.doc.Name)

            reopened = FreeCAD.openDocument(path)
            try:
                cage = reopened.getObject("Cage")
                skin = reopened.getObject("Skin")
                self.assertEqual(type(cage.Proxy).__name__, "CageProxy")
                self.assertEqual(type(skin.Proxy).__name__, "SkinProxy")
                self.assertEqual(int(cage.Levels), 1)
                self.assertEqual(len(cage.CagePoints), 8)

                reopened.recompute()
                # The proxies rebuilt the same solid from the restored cage.
                self.assertAlmostEqual(
                    reopened.getObject("Body").Shape.Volume, volume, places=6
                )
                self.assertEqual(
                    reopened.getObject("Body").BaseFeature.Name, "Skin"
                )
            finally:
                FreeCAD.closeDocument(reopened.Name)

    def test_attach_reinstalls_the_proxies_after_an_fcstd_load(self):
        _cage, _skin, body = self.build(levels=1)
        volume = body.Shape.Volume
        with tempfile.TemporaryDirectory() as tmp:
            path = os.path.join(tmp, "sculpt.FCStd")
            self.doc.saveAs(path)
            FreeCAD.closeDocument(self.doc.Name)

            reopened = FreeCAD.openDocument(path)
            try:
                # `.FCStd` keeps the properties and the shapes but not the proxy
                # (the writer reports upstream's binary Python object instead of
                # inventing one), so the workbench re-attaches.
                self.assertIsNone(reopened.getObject("Cage").Proxy)
                attached = Sculpt.attach(reopened)
                self.assertEqual(sorted(attached), ["Cage", "Skin"])
                reopened.recompute()
                self.assertAlmostEqual(
                    reopened.getObject("Body").Shape.Volume, volume, places=6
                )
            finally:
                FreeCAD.closeDocument(reopened.Name)


if __name__ == "__main__":
    unittest.main()
