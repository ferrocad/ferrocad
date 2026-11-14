"""P3 host spike: the calls the Curves and SubD workbenches make, headless.

Neither add-on is in this checkout, so this module **synthesizes their call
surface** from the object model their sources document, and runs it against
FerroCAD. Every call below is upstream FreeCAD API; a failure is therefore a
named gap, not a missing feature of the spike.

The two workloads:

* **SubD Modeler** (the closest worked answer to a freeform skin) stores its cage
  on a ``Mesh::FeaturePython`` object (``CagePoints`` + ``CageFaces``), previews
  the Catmull-Clark surface as a mesh, builds each quad as an untrimmed patch with
  ``Part.BSplineSurface.buildFromPolesMultsKnots(...).toShape()``, assembles the
  patches with ``Part.Shell`` + ``Shape.sewShape`` + ``Part.Solid``, and wraps the
  result in a ``Part::FeaturePython`` with an ``App::PropertyLink`` back to the
  cage that rebuilds ``Shape`` in ``execute``.
* **Curves** interpolates B-spline curves through picked points and fills the
  boundary with ``Part.makeFilledFace``.

The design argument is ``docs/project/ideation/surface-and-freeform-modelling.md``
§7; the recorded outcome is ``docs/project/research/p3-addon-surface-spike.md``.
"""

import unittest

import FreeCAD as App
import Mesh
import Part

V = App.Vector


# --- The bundles' proxies -----------------------------------------------------
#
# Defined at module scope so the document's `Proxy` is a real class, as the
# workbenches do. `execute` is the only hook these spikes need.

class CageProxy:
    """The SubD cage: points in, a triangle preview out."""

    def __init__(self):
        self.executes = 0

    def execute(self, obj):
        self.executes += 1
        points = list(obj.CagePoints)
        faces = list(obj.CageFaces)
        if len(points) < 3 or len(faces) < 3:
            obj.Mesh = Mesh.Mesh()
            return
        triangles = [tuple(faces[i:i + 3]) for i in range(0, len(faces) - 2, 3)]
        preview = Mesh.Mesh((points, triangles))
        # A new `Mesh` writes its transform into `Placement` (upstream
        # `MeshFeature::onChanged`), so carry the object's placement over or the
        # cage would jump back to the origin on every cage edit.
        preview.Placement = obj.Placement
        obj.Mesh = preview


class SkinProxy:
    """The surface-to-solid wrapper: follow the cage, rebuild `Shape`."""

    def __init__(self):
        self.executes = 0

    def execute(self, obj):
        self.executes += 1
        cage = obj.Cage
        if cage is None:
            obj.Shape = Part.Shape()
            return
        box = cage.Mesh.BoundBox
        size = max(box.XLength, box.YLength, 1.0)
        shape = Part.makeBox(size, size, size)
        shape.translate(cage.Placement.Base)
        obj.Shape = shape


def bicubic_patch(corner, u, v):
    """One untrimmed 3x3 patch, as the SubD workbench builds each quad.

    ``u``/``v`` are the full side vectors; the 4x4 pole grid spans the patch,
    clamped with multiplicity 4 at each end (degree 3 in both directions).
    """
    origin = V(*corner)
    u = V(*u)
    v = V(*v)
    poles = [[origin + u * (i / 3.0) + v * (j / 3.0) for j in range(4)] for i in range(4)]
    surface = Part.BSplineSurface()
    surface.buildFromPolesMultsKnots(poles, [4, 4], [4, 4], udegree=3, vdegree=3)
    return surface.toShape()


class SpikeCase(unittest.TestCase):
    def setUp(self):
        self.doc = App.newDocument("P3AddonSpike")

    def tearDown(self):
        App.closeDocument(self.doc.Name)


class CageTests(SpikeCase):
    def test_the_cage_lives_on_a_mesh_feature_python(self):
        """`import PartDesign`'s sibling: the document type is what was missing."""
        cage = self.doc.addObject("Mesh::FeaturePython", "Cage", CageProxy(), None)
        cage.addProperty("App::PropertyVectorList", "CagePoints", "Cage")
        cage.addProperty("App::PropertyIntegerList", "CageFaces", "Cage")
        cage.CagePoints = [V(0, 0, 0), V(10, 0, 0), V(0, 10, 0)]
        cage.CageFaces = [0, 1, 2]

        self.doc.recompute()
        self.assertEqual(cage.Proxy.executes, 1)
        self.assertEqual(cage.Mesh.CountPoints, 3)
        self.assertEqual(cage.Mesh.CountFacets, 1)
        self.assertAlmostEqual(cage.Mesh.Area, 50.0, places=6)

        # A cage edit re-runs the proxy and moves the preview.
        cage.CagePoints = [V(0, 0, 0), V(20, 0, 0), V(0, 10, 0)]
        self.doc.recompute()
        self.assertEqual(cage.Proxy.executes, 2)
        self.assertAlmostEqual(cage.Mesh.Area, 100.0, places=6)


class PatchTests(SpikeCase):
    def test_a_quad_becomes_a_bicubic_patch(self):
        face = bicubic_patch((0, 0, 0), (1, 0, 0), (0, 1, 0))
        self.assertEqual(face.ShapeType, "Face")
        self.assertTrue(face.isValid())
        self.assertIsInstance(face.Surface, Part.BSplineSurface)
        self.assertEqual((face.Surface.UDegree, face.Surface.VDegree), (3, 3))
        self.assertAlmostEqual(face.Area, 1.0, places=9)

    def test_patches_sew_into_a_closed_solid(self):
        """The six-patch skin: `Part.Shell` + `sewShape` + `Part.Solid`."""
        patches = [
            bicubic_patch((0, 0, 0), (1, 0, 0), (0, 1, 0)),
            bicubic_patch((0, 0, 1), (1, 0, 0), (0, 1, 0)),
            bicubic_patch((0, 0, 0), (1, 0, 0), (0, 0, 1)),
            bicubic_patch((0, 1, 0), (1, 0, 0), (0, 0, 1)),
            bicubic_patch((0, 0, 0), (0, 1, 0), (0, 0, 1)),
            bicubic_patch((1, 0, 0), (0, 1, 0), (0, 0, 1)),
        ]
        self.assertTrue(all(face.isValid() for face in patches))

        shell = Part.Shell(patches)
        shell.sewShape(1e-3)
        self.assertEqual(len(shell.Faces), 6)
        self.assertTrue(shell.isClosed(), "the sewn skin is a closed shell")
        self.assertTrue(shell.isValid())

        solid = Part.Solid(shell)
        self.assertEqual(len(solid.Solids), 1)
        self.assertTrue(solid.isValid())
        self.assertTrue(solid.isClosed())
        self.assertAlmostEqual(solid.Volume, 1.0, places=6)


class WrapperTests(SpikeCase):
    def test_the_wrapper_rebuilds_its_shape_from_the_cage_link(self):
        cage = self.doc.addObject("Mesh::FeaturePython", "Cage", CageProxy(), None)
        cage.addProperty("App::PropertyVectorList", "CagePoints", "Cage")
        cage.addProperty("App::PropertyIntegerList", "CageFaces", "Cage")
        cage.CagePoints = [V(0, 0, 0), V(12, 0, 0), V(0, 12, 0)]
        cage.CageFaces = [0, 1, 2]

        skin = self.doc.addObject("Part::FeaturePython", "Skin", SkinProxy(), None)
        skin.addProperty("App::PropertyLink", "Cage", "Surface")
        skin.addProperty("App::PropertyInteger", "Samples", "Surface")
        skin.addProperty("App::PropertyFloat", "Tolerance", "Surface")
        skin.Cage = cage
        skin.Samples = 8
        skin.Tolerance = 1e-3

        self.doc.recompute()
        self.assertEqual(skin.Proxy.executes, 1)
        self.assertFalse(skin.Shape.isNull())
        self.assertTrue(skin.Shape.isValid())
        self.assertEqual(len(skin.Shape.Solids), 1)

        # The wrapper follows the cage's Placement, as the workbench documents.
        cage.Placement = App.Placement(V(50, 0, 0), App.Rotation())
        self.doc.recompute()
        self.assertEqual(skin.Proxy.executes, 2)
        self.assertAlmostEqual(skin.Shape.BoundBox.XMin, 50.0, places=6)


class CurvesTests(SpikeCase):
    def test_a_b_spline_curve_interpolates_and_fills(self):
        points = [V(0, 0, 0), V(10, 5, 0), V(20, 0, 0), V(10, -5, 0)]
        curve = Part.BSplineCurve()
        curve.interpolate(points, PeriodicFlag=True)
        edge = curve.toShape()
        self.assertEqual(edge.ShapeType, "Edge")
        self.assertGreater(edge.Length, 0.0)

        wire = Part.Wire([edge])
        self.assertTrue(wire.isClosed())
        face = Part.makeFilledFace(wire.Edges)
        self.assertEqual(face.ShapeType, "Face")
        self.assertTrue(face.isValid())
        self.assertGreater(face.Area, 0.0)


if __name__ == "__main__":
    unittest.main()
