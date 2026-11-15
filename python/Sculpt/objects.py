"""The two document objects the Sculpt workbench edits.

**The cage** is a ``Mesh::FeaturePython``: ``CagePoints`` and ``CageFaces`` are
the control hull, and ``execute`` writes the Catmull-Clark preview to ``Mesh``.
A cage edit therefore shows up as a new preview mesh without a B-rep rebuild —
that is what makes dragging a point feel like a mesh operation.

**The skin** is a ``Part::FeaturePython`` with a link back to the cage: it
subdivides the same cage, builds one bicubic patch per quad, sews the shell and
stores the solid in ``Shape``. It follows the cage's ``Placement`` and its edits,
so the two objects are one model with two representations.

Neither proxy calls into the other; the document's link graph orders them.
"""

import Mesh
import Part

import FreeCAD

from .patches import patch_solid
from .subdivision import cage_quads, catmull_clark, triangles


def is_cage(obj):
    """Whether ``obj`` is a Sculpt cage (a `Mesh::FeaturePython` with `CagePoints`)."""
    return (
        obj is not None
        and getattr(obj, "TypeId", "") == "Mesh::FeaturePython"
        and obj.getPropertyByName("CagePoints") is not None
    )


def is_skin(obj):
    """Whether ``obj`` is a Sculpt skin (a `Part::FeaturePython` linking a cage)."""
    return (
        obj is not None
        and getattr(obj, "TypeId", "") == "Part::FeaturePython"
        and obj.getPropertyByName("Cage") is not None
        and obj.getPropertyByName("Tolerance") is not None
    )


class CageProxy:
    """`execute` turns the control hull into a Catmull-Clark preview mesh."""

    def execute(self, obj):
        points = list(obj.CagePoints)
        faces = cage_quads(obj.CageFaces)
        if len(points) < 4 or not faces:
            obj.Mesh = Mesh.Mesh()
            return
        for _ in range(int(obj.Levels)):
            points, faces = catmull_clark(points, faces)
        preview = Mesh.Mesh((points, triangles(faces)))
        # A new `Mesh` hands its own transform to the object, so carry the
        # object's placement over or the cage would jump to the origin.
        preview.Placement = obj.Placement
        obj.Mesh = preview

    def onDocumentRestored(self, obj):
        self.execute(obj)


class SkinProxy:
    """`execute` turns the same hull into the B-rep the body consumes."""

    def execute(self, obj):
        cage = obj.Cage
        if cage is None or not hasattr(cage, "CagePoints"):
            obj.Shape = Part.Shape()
            return
        points = list(cage.CagePoints)
        faces = cage_quads(cage.CageFaces)
        if len(points) < 4 or not faces:
            obj.Shape = Part.Shape()
            return
        # `Levels = 0` follows the cage, so the two representations agree
        # unless the user deliberately gives the skin its own resolution.
        levels = int(obj.Levels) if int(obj.Levels) > 0 else int(cage.Levels)
        for _ in range(levels):
            points, faces = catmull_clark(points, faces)
        # The cage's own placement is part of the hull: the skin's `Shape` is in
        # the cage's frame (the object's `Placement` is then applied on top by
        # the Part feature, exactly as for any Python feature).
        placement = cage.Placement
        placed = [placement.multVec(point) for point in points]
        obj.Shape = patch_solid(placed, faces, float(obj.Tolerance))

    def onDocumentRestored(self, obj):
        self.execute(obj)
