"""Sculpt: a quad cage in, an editable solid out, nested in a ``PartDesign::Body``.

The bundle is three pieces that the rest of this module wires together:

``Sculpt.subdivision``
    Catmull-Clark refinement of a polygon cage, with the boundary rules, so an
    open cage keeps its rim while it is being built.

``Sculpt.objects``
    The two document objects. The **cage** is a ``Mesh::FeaturePython`` holding
    ``CagePoints``/``CageFaces`` and previewing the subdivision as a mesh; the
    **skin** is a ``Part::FeaturePython`` that subdivides the same cage into one
    bicubic patch per quad and sews them into a solid.

``Sculpt.nest``
    The body adapter. A ``PartDesign::Body`` accepts the cage as a **support**
    member (it is not the body's solid), adopts the skin as its ``BaseFeature``
    and lets PartDesign features take the ``Tip`` on top of it.

Example::

    import FreeCAD, PartDesign, Sculpt

    doc = FreeCAD.newDocument("SculptDemo")
    points, faces = Sculpt.box_cage(40, 30, 20)
    cage = Sculpt.makeCage(doc, "Cage", points, faces, levels=2)
    skin = Sculpt.makeSkin(doc, "Skin", cage)
    body = doc.addObject("PartDesign::Body", "Body")
    Sculpt.nest(body, cage, skin)
    doc.recompute()

``nest`` answers the role each object took (``"support"``, ``"base"``,
``"tip"``), which is the one thing about body membership a workbench has to get
right: the cage must not become the body's shape, and the skin must be the
shape the features build on.

Persistence needs nothing extra: a proxy is stored by its module and class name,
so a reopened document rebuilds both objects through ``onDocumentRestored``. The
GUI commands are not part of this slice — the workbench is headless.
"""

import FreeCAD

from .objects import CageProxy, SkinProxy
from .patches import bicubic_patch, patch_shell, patch_solid
from .subdivision import cage_quads, catmull_clark, triangles

#: Extra subdivision levels used when an object does not say otherwise.
DEFAULT_LEVELS = 2

#: The sewing tolerance of the patch shell (mm).
DEFAULT_TOLERANCE = 1e-3


def subdivide(points, faces, levels=1):
    """Catmull-Clark ``levels`` times, returning ``(points, quads)``."""
    for _ in range(int(levels)):
        points, faces = catmull_clark(points, faces)
    return points, faces


def box_cage(length, width, height, origin=(0.0, 0.0, 0.0)):
    """A six-quad box cage, outward wound, as ``(points, flat_faces)``.

    The smallest cage that is closed, so it is what the tests and the user page
    start from; a real cage is dragged into shape afterwards.
    """
    x, y, z = (float(value) for value in origin)
    length, width, height = float(length), float(width), float(height)
    points = [
        FreeCAD.Vector(x, y, z),
        FreeCAD.Vector(x + length, y, z),
        FreeCAD.Vector(x + length, y + width, z),
        FreeCAD.Vector(x, y + width, z),
        FreeCAD.Vector(x, y, z + height),
        FreeCAD.Vector(x + length, y, z + height),
        FreeCAD.Vector(x + length, y + width, z + height),
        FreeCAD.Vector(x, y + width, z + height),
    ]
    faces = [
        (0, 3, 2, 1),  # bottom (-Z)
        (4, 5, 6, 7),  # top (+Z)
        (0, 1, 5, 4),  # front (-Y)
        (1, 2, 6, 5),  # right (+X)
        (2, 3, 7, 6),  # back (+Y)
        (3, 0, 4, 7),  # left (-X)
    ]
    return points, [index for face in faces for index in face]


def makeCage(document, name, points, faces, levels=DEFAULT_LEVELS):
    """Add a cage object: ``CagePoints``, ``CageFaces`` and a mesh preview."""
    cage = document.addObject("Mesh::FeaturePython", name, CageProxy(), None)
    cage.addProperty("App::PropertyVectorList", "CagePoints", "Cage")
    cage.addProperty("App::PropertyIntegerList", "CageFaces", "Cage")
    cage.addProperty("App::PropertyInteger", "Levels", "Cage")
    cage.CagePoints = [FreeCAD.Vector(point) for point in points]
    cage.CageFaces = list(faces)
    cage.Levels = int(levels)
    return cage


def makeSkin(document, name, cage, levels=0, tolerance=DEFAULT_TOLERANCE):
    """Add the link back to ``cage`` and the patch solid it builds.

    ``levels = 0`` follows the cage's own resolution, so the preview and the
    solid agree; a positive value gives the skin its own.
    """
    if cage is None or not hasattr(cage, "CagePoints"):
        raise ValueError("makeSkin needs a cage object")
    skin = document.addObject("Part::FeaturePython", name, SkinProxy(), None)
    skin.addProperty("App::PropertyLink", "Cage", "Surface")
    skin.addProperty("App::PropertyInteger", "Levels", "Surface")
    skin.addProperty("App::PropertyFloat", "Tolerance", "Surface")
    skin.Cage = cage
    skin.Levels = int(levels)
    skin.Tolerance = float(tolerance)
    return skin


def role_of(body, obj):
    """What ``obj`` currently is inside ``body``: ``"tip"``, ``"base"`` or
    ``"support"``.

    ``BaseFeature`` is checked first: a body back-fills ``Tip`` with its base when
    it has no feature on top, and the base role is the meaningful one then.
    """
    name = obj.Name
    base = body.BaseFeature
    if base is not None and base.Name == name:
        return "base"
    tip = body.Tip
    if tip is not None and tip.Name == name:
        return "tip"
    return "support"


def attach(document):
    """Install the bundle's proxies on objects that arrived without one.

    The native format keeps a proxy; a ``.FCStd`` does not (upstream stores a
    binary Python object there, which FerroCAD reports rather than writes). A
    workbench calls this after opening a ``.FCStd`` so the cage preview and the
    skin can rebuild; it returns the names it attached to.
    """
    attached = []
    for obj in document.Objects:
        try:
            if obj.Proxy is not None:
                continue
        except AttributeError:
            continue
        if obj.TypeId == "Mesh::FeaturePython" and (
            obj.getPropertyByName("CagePoints") is not None
        ):
            obj.Proxy = CageProxy()
            attached.append(obj.Name)
        elif (
            obj.TypeId == "Part::FeaturePython"
            and obj.getPropertyByName("Cage") is not None
            and obj.getPropertyByName("Tolerance") is not None
        ):
            obj.Proxy = SkinProxy()
            attached.append(obj.Name)
    return attached


def nest(body, *objects):
    """Add ``objects`` to ``body`` and return the role each one took.

    The order matters: ``BaseFeature`` only accepts its base while the body holds
    no solid, so add the skin before any PartDesign feature builds on it.
    """
    roles = []
    for obj in objects:
        body.addObject(obj)
        roles.append(role_of(body, obj))
    return roles


__all__ = [
    "CageProxy",
    "DEFAULT_LEVELS",
    "DEFAULT_TOLERANCE",
    "SkinProxy",
    "attach",
    "bicubic_patch",
    "box_cage",
    "cage_quads",
    "catmull_clark",
    "makeCage",
    "makeSkin",
    "nest",
    "patch_shell",
    "patch_solid",
    "role_of",
    "subdivide",
    "triangles",
]
