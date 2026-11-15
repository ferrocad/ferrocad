"""App-shell script for **FerroCAD: SimpleCAD**.

The edition is the Parametric composition plus the canvas-first tools: the same
shell data surface and the same Part display and interaction providers (a
SimpleCAD face *is* a ``Part::Feature``), with its own command set on top.
Re-exporting ``ferrocad_parametric`` rather than copying it keeps one definition
of "how a Part object is drawn and edited".

What this script adds:

* ``SimpleCAD_Rectangle``, ``SimpleCAD_Circle``, ``SimpleCAD_Polygon`` and
  ``SimpleCAD_Arc`` — arm the native drawing tools. The tools are Rust
  (``ferrocad_stage::RectangleTool`` and friends), so they preview on the viewport
  and complete themselves on the last press; this hook owns only what a commit
  *means*.
* the ``rectangle``, ``circle``, ``polygon`` and ``arc`` ``create`` interactions: the
  tool's world geometry in, one ``Part::Feature`` out, in one named transaction,
  through SimpleCAD's own builders (``simplecad.core.geometry``) when that package is
  importable.

That split is the point of the edition: **the tool collects world geometry, the
script names the document object**. Every later SimpleCAD tool takes the same shape.

The shell imports this module by name (the edition's ``HostConfig.entry_module``),
so it must re-export every entry point the shell calls.
"""

from __future__ import annotations

import math

import FreeCAD
import FreeCADGui
import FerroCAD

# The shell's data functions (`bootstrap`, `model_tree`, `properties`,
# `set_property`, `evaluate`, `stage_meshes`, `interaction`, ...), the Part
# providers and the Parametric commands all arrive through these two imports.
from ferrocad_parametric import *  # noqa: F401,F403
from ferrocad_parametric import _add_appearance, _invalid
from ferrocad_parametric import bootstrap as _parametric_bootstrap

import Part  # noqa: F401  (links the kernel; registers Part::Feature)

try:  # The modelling layer lives in the sibling checkout or an installed add-on.
    from simplecad.core import geometry as _simplecad_geometry
except Exception:  # noqa: BLE001 - the edition works without the package
    _simplecad_geometry = None

#: The tool ids the edition registers in its `Contributions` (see `src/lib.rs`).
RECTANGLE_TOOL = "rectangle"
CIRCLE_TOOL = "circle"
POLYGON_TOOL = "polygon"
ARC_TOOL = "arc"
#: The action every drawing tool's commit carries.
CREATE = "create"

#: The chrome surface this edition's commands sit on.
SURFACE = "SimpleCAD"
#: Upstream's polygon ceiling (`geometry.MAX_POLYGON_SIDES`).
MAX_POLYGON_SIDES = 100


def _vector(values, index):
    """Three numbers from a commit, as a world vector."""
    return FreeCAD.Vector(*values[index : index + 3])


def _rectangle_plane(corners):
    """The frame the four corners span: `(normal, u, v)`.

    The corners are what the tool committed, in order, so the frame is derived from
    them rather than re-derived from a rule: `u` runs along the first side and `v`
    along the last, which is exactly the box the user previewed.
    """
    origin = corners[0]
    u = corners[1].sub(origin)
    v = corners[3].sub(origin)
    if u.Length <= 0.0 or v.Length <= 0.0:
        raise ValueError("a rectangle needs two sides")
    u = u.normalize()
    v = v.normalize()
    return u.cross(v).normalize(), u, v


def _face_from_corners(corners):
    """The planar face through four corners, in their own winding.

    The SimpleCAD package's ``geometry.rectangle`` is the upstream-equivalent builder
    and is used whenever it is importable, so the object model matches its drawing
    tools. Without the package the same six lines run on `Part` directly — the corners
    are already exact, so the face cannot differ (the round trip asserts they agree).
    """
    normal, u, v = _rectangle_plane(corners)
    if _simplecad_geometry is not None:
        plane = _simplecad_geometry.Plane(corners[0], u, v, normal)
        return _simplecad_geometry.rectangle(plane, corners[0], corners[2])
    wire = Part.makePolygon(list(corners) + [corners[0]])
    return Part.Face(wire)


def _frame(center, radial, normal):
    """The drawing frame a circle or a polygon was committed in: `(plane, u, n, r)`.

    The tool sends the radial point plus the plane's normal, so this is the geometry
    the user previewed rather than a rule re-derived from it.
    """
    radius = radial.Length
    if radius < 1e-9:
        raise ValueError("a circle or polygon needs a positive radius")
    if normal.Length < 1e-9:
        raise ValueError("a circle or polygon needs a drawing plane")
    u = radial / radius
    normal = normal / normal.Length
    plane = (
        _simplecad_geometry.Plane(center, u, normal.cross(u), normal)
        if _simplecad_geometry is not None
        else None
    )
    return plane, u, normal, radius


def _circle_face(values):
    """A circle from `(centre, rim, normal)`: the package's builder, else `Part`."""
    center, rim, normal = _vector(values, 0), _vector(values, 3), _vector(values, 6)
    plane, _, normal, radius = _frame(center, rim - center, normal)
    if _simplecad_geometry is not None:
        return _simplecad_geometry.circle(plane, center, radius)
    return Part.Face(Part.Wire([Part.makeCircle(radius, center, normal)]))


def _polygon_face(values):
    """A regular polygon from `(centre, vertex, normal, sides)`."""
    center, vertex, normal = _vector(values, 0), _vector(values, 3), _vector(values, 6)
    sides = int(round(values[9]))
    if not 3 <= sides <= MAX_POLYGON_SIDES:
        raise ValueError(
            "a polygon needs between 3 and %d sides" % MAX_POLYGON_SIDES
        )
    plane, u, normal, radius = _frame(center, vertex - center, normal)
    if _simplecad_geometry is not None:
        return _simplecad_geometry.polygon(plane, center, vertex, sides)
    v = normal.cross(u)
    points = [
        center
        + u * (radius * math.cos(index * math.tau / sides))
        + v * (radius * math.sin(index * math.tau / sides))
        for index in range(sides)
    ]
    return Part.Face(Part.makePolygon(points + points[:1]))


def _arc_edge(values):
    """An arc edge from three points on it: start, halfway, end.

    Three points on a circle are exactly what upstream's ``Arc.edge()`` hands the
    kernel, so the arc is the same curve without any plane arithmetic here.
    """
    start, mid, end = _vector(values, 0), _vector(values, 3), _vector(values, 6)
    edge = Part.Arc(start, mid, end).toShape()
    if not edge.isValid():
        raise ValueError("this arc could not be created; increase its radius")
    return edge


def _commit(name, shape_of, values, expected):
    """One drawing commit: a builder, one named transaction, one `Part::Feature`.

    `expected` is how many numbers the tool commits; a short list is refused before
    anything is written, so a malformed commit cannot leave a half-built object.
    """
    if len(values) < expected:
        return {"ok": False, "error": "%s needs %d numbers" % (name, expected)}
    document = FreeCAD.ActiveDocument
    if document is None:
        document = FreeCAD.newDocument("Unnamed")
    document.openTransaction(name)
    try:
        feature = document.addObject("Part::Feature", name)
        feature.Shape = shape_of(values)
        _add_appearance(feature)
        document.recompute()
        if _invalid(feature):
            document.abortTransaction()
            return {"ok": False, "error": "the %s could not be built" % name.lower()}
        document.commitTransaction()
    except Exception as exc:  # noqa: BLE001 - one bad commit must not break the shell
        document.abortTransaction()
        return {"ok": False, "error": str(exc)}
    return {"ok": True, "feature": feature.Name}


def _from_corners(values):
    """The twelve committed numbers as the planar face they name."""
    corners = [_vector(values, index) for index in range(0, 12, 3)]
    return _face_from_corners(corners)


#: Each drawing tool id, and what its `create` commit means: the object's name, the
#: builder and how many numbers the commit must carry. One table, so a tool added to
#: `src/lib.rs` without a meaning here shows up as `None` rather than a wrong object.
_BUILDERS = {
    RECTANGLE_TOOL: ("Rectangle", _from_corners, 12),
    CIRCLE_TOOL: ("Circle", _circle_face, 9),
    POLYGON_TOOL: ("Polygon", _polygon_face, 10),
    ARC_TOOL: ("Arc", _arc_edge, 9),
}


def _interaction(obj, manipulator, action, element, values):
    """Apply one of this edition's manipulator commits, or defer (`None`).

    Only this edition's drawing tools are ours; every other manipulator belongs to
    the Parametric script (the dress-up scrubber, the push/pull stem, the sketch
    grips), whose provider runs beside this one.

    The name is underscored on purpose: ``interaction`` is the *shell* entry point
    (``ferrocad_shell.interaction``) the Python host calls by name, and shadowing it
    would hand this hook a JSON string where it expects a list.
    """
    if action != CREATE:
        return None
    builder = _BUILDERS.get(manipulator)
    if builder is None:
        return None
    name, shape_of, expected = builder
    return _commit(name, shape_of, [float(value) for value in values], expected)


FerroCAD.register_interaction_provider(_interaction)


class _DrawCommand:
    """A ``SimpleCAD_*`` command that arms one of the edition's drawing tools.

    The command only arms the tool; the drawing, the preview and the commit are the
    tool's, and `_interaction` above turns the commit into an object. Subclasses say
    which tool they arm and how they read in the menus.
    """

    #: The tool id this command arms (see `src/lib.rs`).
    TOOL = ""
    #: The menu text.
    MENU = ""
    #: The one-line tooltip: what the user clicks.
    TIP = ""

    def GetResources(self):
        return {
            "MenuText": self.MENU,
            "ToolTip": self.TIP,
            "Group": SURFACE,
            "CmdType": "ForEdit",
            "FerroCAD_Surface": SURFACE,
        }

    def IsActive(self):
        return True

    def Activated(self, iMsg=0):
        return {"tool": self.TOOL}


class _RectangleCommand(_DrawCommand):
    """``SimpleCAD_Rectangle`` — draw a rectangle in two clicks."""

    TOOL = RECTANGLE_TOOL
    MENU = "Rectangle"
    TIP = "Click two opposite corners to draw a rectangle face"


class _CircleCommand(_DrawCommand):
    """``SimpleCAD_Circle`` — draw a circle in two clicks."""

    TOOL = CIRCLE_TOOL
    MENU = "Circle"
    TIP = "Click the centre, then the radius, to draw a circle face"


class _PolygonCommand(_DrawCommand):
    """``SimpleCAD_Polygon`` — draw a regular polygon in two clicks."""

    TOOL = POLYGON_TOOL
    MENU = "Polygon"
    TIP = "Click the centre, then a vertex, to draw a regular polygon face"


class _ArcCommand(_DrawCommand):
    """``SimpleCAD_Arc`` — draw a circular arc in three clicks."""

    TOOL = ARC_TOOL
    MENU = "Arc"
    TIP = "Click the centre, the start, then the end, to draw an arc"


for _name, _command in (
    ("Rectangle", _RectangleCommand()),
    ("Circle", _CircleCommand()),
    ("Polygon", _PolygonCommand()),
    ("Arc", _ArcCommand()),
):
    FreeCADGui.addCommand("SimpleCAD_" + _name, _command)
del _name, _command


def bootstrap(doc_name: str = "Sample") -> str:
    """Open the canonical sample and report host status.

    The edition seeds no demo of its own: the canvas-first workflow is drawing, and a
    seeded face would only be one more object to select around.
    """
    return _parametric_bootstrap(doc_name)
