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
    from simplecad.document import operations as _simplecad_operations
    from simplecad.surfaces import offset as _simplecad_offset
except Exception:  # noqa: BLE001 - the edition works without the package
    _simplecad_geometry = None
    _simplecad_operations = None
    _simplecad_offset = None

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


# ---------------------------------------------------------------------------
# The element verbs: Offset, Elongate, Rotate
#
# SimpleCAD's `PICK` family works on **one** picked sub-element and a value. Two of
# them (`Offset`, `Elongate`) are the surface/edge siblings of Push/Pull, and the
# third (`Rotate`) turns a drawing's own arc. They are declared here as handle specs,
# which is how this edition contributes an interactive drag without Rust: the host
# already arms a declarative handle, publishes its docked numeric field and routes the
# commit back to `_interaction`.
#
# ## One pick, one handle
#
# The two compiled manipulators own the rest of the space, and the partition is by
# element kind, so no pick is ever claimed twice:
#
# | Pick | Owner | Why |
# | --- | --- | --- |
# | planar `FaceN` | `PushPullStem` (slot 2) | an offset of a plane *is* push/pull |
# | `EdgeN` of a body with faces | `DressUpScrubber` (slot 3) | a dress-up is the edge edit |
# | **curved** `FaceN` | this file (`Offset`/`Elongate`) | the stem refuses a non-planar face |
# | `EdgeN` of an item with **no** faces | this file (`Offset`/`Elongate`/`Rotate`) | the scrubber refuses a faceless item |
#
# A **command** chooses which verb the faceless cases get (`SimpleCAD_Offset`,
# `_Elongate`, `_Rotate`); the default is `Offset`. The mode is sticky, like SimpleCAD's
# `Draw: Surface` switch, and the selection re-arms on the next pick, which is when the
# handle appears.
# ---------------------------------------------------------------------------

#: Offset a face or a drawing edge outward/inward by a distance.
OFFSET = "offset"
#: Stretch a circular rim or a curved face into an ellipse.
ELONGATE = "elongate"
#: Turn a drawing's own arc about its centre.
ROTATE = "rotate"
#: The verb a faceless edge gets before a command arms another.
_ARMED = OFFSET
#: The declared handle's registry id; the commit's ``manipulator``.
ELEMENT_HANDLE = "simplecad_element"
#: A side shorter than this is not an offset/elongation worth committing.
_ELEMENT_MIN = 1e-3


def _arm(verb):
    """Arm one element verb for the next pick, and say so."""
    global _ARMED
    _ARMED = verb
    return {"status": "%s armed: pick a face or a drawing edge" % verb}


def _curved(face):
    """Whether ``face`` is one the compiled push/pull stem declines (not a plane)."""
    try:
        return not isinstance(face.Surface, Part.Plane)
    except Exception:  # noqa: BLE001 - a face without a surface is not claimable
        return False


def _circular(edge):
    """Whether ``edge`` lies on a circle (a rim, an arc)."""
    try:
        return isinstance(edge.Curve, Part.Circle)
    except Exception:  # noqa: BLE001 - an edge without a curve is not claimable
        return False


def _unit(vector):
    """``vector`` scaled to length one, or ``None`` when it is too short."""
    length = vector.Length
    if length < 1e-9:
        return None
    return vector / length


def _edge_frame(edge, radial):
    """A handle frame at ``edge``'s midpoint.

    ``radial`` places ``z`` along the rim's own radius (an offset/elongation drags
    outward); otherwise ``z`` is the circle's axis (a rotation turns about the centre).
    """
    curve = edge.Curve
    try:
        center = curve.Center
        axis = _unit(curve.Axis)
    except Exception:  # noqa: BLE001 - a freeform edge has no circle frame
        return None
    middle = edge.valueAt(0.5 * (edge.FirstParameter + edge.LastParameter))
    if radial:
        z = _unit(middle - center)
        origin = middle
    else:
        z = axis
        origin = center
    if z is None:
        return None
    helper = FreeCAD.Vector(1, 0, 0) if abs(z.x) < 0.9 else FreeCAD.Vector(0, 1, 0)
    x = _unit(helper.cross(z))
    if x is None:
        return None
    y = z.cross(x)
    return {"origin": list(origin), "x": list(x), "y": list(y), "z": list(z)}


def _surface_frame(face):
    """A handle frame on a face, read from the face's own surface.

    The display mesh cannot be used for a **closed** curved face: the outward normals of
    a whole cylinder cancel, so an area-weighted sum names no normal at all. The surface
    can: the centroid is `CenterOfMass` and the normal is `normalAt` in the middle of its
    parameter range.
    """
    try:
        first, last, bottom, top = face.ParameterRange
        z = _unit(face.normalAt(0.5 * (first + last), 0.5 * (bottom + top)))
        origin = face.CenterOfMass
    except Exception:  # noqa: BLE001 - a face without a frame offers no handle
        return None
    if z is None:
        return None
    helper = FreeCAD.Vector(1, 0, 0) if abs(z.x) < 0.9 else FreeCAD.Vector(0, 1, 0)
    x = _unit(helper.cross(z))
    if x is None:
        return None
    y = z.cross(x)
    return {"origin": list(origin), "x": list(x), "y": list(y), "z": list(z)}


def _element_specs(obj, element=""):
    """The one declared handle this pick offers, or ``None``.

    The two declines are the whole disambiguation: a **planar** face belongs to
    ``PushPullStem`` and an **edge of a face-bearing body** to the ``Fillet``
    ``DressUpScrubber``. Everything answered here is a pick those two refuse, so two
    manipulators never fight over the same element (see the table above).
    """
    if not element or _simplecad_operations is None:
        return None
    shape = getattr(obj, "Shape", None)
    if shape is None:
        return None
    try:
        picked = shape.getElement(element)
    except Exception:  # noqa: BLE001 - an element the shape cannot resolve
        return None

    if element.startswith("Face"):
        if not _curved(picked):
            return None
        frame = _surface_frame(picked)
        # A face takes the two verbs that move it along its own normal; a rotate is a
        # drawing verb, so an armed `Rotate` leaves the face to `Offset`.
        verb = ELONGATE if _ARMED == ELONGATE else OFFSET
        label, readout = "D", "D {value} mm"
    elif element.startswith("Edge"):
        if len(shape.Faces) or not _circular(picked):
            return None
        frame = _edge_frame(picked, radial=_ARMED != ROTATE)
        verb = _ARMED
        if verb == ROTATE:
            label, readout = "A", "A {value}°"
        else:
            label, readout = "D", "D {value} mm"
    else:
        return None
    if frame is None:
        return None
    return [
        {
            "id": ELEMENT_HANDLE,
            "action": verb,
            "element": element,
            "frame": frame,
            "axis": "z",
            "kind": "ring" if verb == ROTATE else "axis",
            "value": 0.0,
            "decimals": 3,
            "label": label,
            "size": 42.0,
            "readout": readout,
        }
    ]


def _offset_element(obj, element, distance):
    """Move one face along its normal, or offset a drawing edge, one transaction.

    The two halves are different operations in the modelling layer, not two call styles:
    `surfaces.offset.offset_faces` **moves** the picked faces of the body and returns the
    rebuilt solid — the curved-face edit the stem cannot take — while
    `document.operations.offset` builds the offset *profile* of an edge as a new object.
    The surface half is called as a pure function and committed here, because the
    package's `surfaces.operations` wrapper hands the job to a worker process, which an
    embedded interpreter cannot spawn.
    """
    try:
        if element.startswith("Face"):
            index = int(element[4:]) - 1
            result, _report = _simplecad_offset.offset_faces(obj.Shape, [index], distance)
            document = obj.Document
            document.openTransaction("Offset %s" % element)
            try:
                obj.Shape = result
                document.recompute()
                if _invalid(obj):
                    raise ValueError("the body refused the offset")
                document.commitTransaction()
            except Exception:  # noqa: BLE001 - one transaction, then re-raise
                document.abortTransaction()
                raise
        else:
            _simplecad_operations.offset(obj, element, distance)
    except Exception as exc:  # noqa: BLE001 - one bad commit must not break the shell
        return {"ok": False, "error": str(exc)}
    obj.Document.recompute()
    return {"ok": True}


def _elongate_element(obj, element, distance):
    """Stretch a rim or a curved face into an ellipse along the handle's axis."""
    try:
        direction = _handle_direction(obj, element)
        if direction is None:
            return {"ok": False, "error": "this element has no elongation direction"}
        _simplecad_operations.elongate(
            obj, element, abs(distance), direction, mode="ellipse"
        )
    except Exception as exc:  # noqa: BLE001 - one bad commit must not break the shell
        return {"ok": False, "error": str(exc)}
    obj.Document.recompute()
    return {"ok": True}


def _handle_direction(obj, element):
    """The handle frame's ``z`` for ``element``: the drag's own direction."""
    shape = getattr(obj, "Shape", None)
    if shape is None:
        return None
    try:
        picked = shape.getElement(element)
    except Exception:  # noqa: BLE001 - an element the shape cannot resolve
        return None
    if element.startswith("Face"):
        frame = _surface_frame(picked)
        return FreeCAD.Vector(*frame["z"]) if frame else None
    frame = _edge_frame(picked, radial=True)
    return FreeCAD.Vector(*frame["z"]) if frame else None


def _rotate_element(obj, element, degrees):
    """Turn one drawing edge about its centre, one transaction.

    A drawing object is one profile, so its shape is rebuilt from its own edges with
    the picked one turned; the other edges are carried through unchanged.
    """
    if abs(degrees) < 1e-9:
        return {"ok": True}
    shape = getattr(obj, "Shape", None)
    if shape is None:
        return {"ok": False, "error": "the object has no shape"}
    try:
        picked = shape.getElement(element)
        placement = FreeCAD.Placement(
            picked.Curve.Center, FreeCAD.Rotation(picked.Curve.Axis, degrees)
        )
        moved = picked.copy()
        moved.Placement = placement.multiply(moved.Placement)
        others = [
            edge
            for index, edge in enumerate(shape.Edges, 1)
            if "Edge%d" % index != element
        ]
        rebuilt = Part.makeCompound([moved] + others)
    except Exception as exc:  # noqa: BLE001 - one bad commit must not break the shell
        return {"ok": False, "error": str(exc)}
    document = obj.Document
    document.openTransaction("Rotate %s" % element)
    try:
        obj.Shape = rebuilt
        document.recompute()
        if _invalid(obj):
            raise ValueError("the body refused the edit")
        document.commitTransaction()
    except Exception as exc:  # noqa: BLE001 - one transaction, then report
        document.abortTransaction()
        return {"ok": False, "error": str(exc)}
    return {"ok": True}


def _element_interaction(obj, manipulator, action, element, values):
    """Apply one element-verb commit, or defer (`None`)."""
    if manipulator != ELEMENT_HANDLE or not values:
        return None
    value = float(values[0])
    if action == OFFSET:
        return _offset_element(obj, element, value)
    if action == ELONGATE:
        return _elongate_element(obj, element, value)
    if action == ROTATE:
        return _rotate_element(obj, element, value)
    return None


FerroCAD.register_manipulator_provider(_element_specs)


def _interaction(obj, manipulator, action, element, values):
    """Apply one of this edition's commits, or defer (`None`).

    Two families are ours: the drawing tools' `create` commits (below) and the element
    verbs' `set` commits (`_element_interaction`). Every other manipulator belongs to
    the Parametric script (the dress-up scrubber, the push/pull stem, the sketch
    grips), whose provider runs beside this one.

    The name is underscored on purpose: ``interaction`` is the *shell* entry point
    (``ferrocad_shell.interaction``) the Python host calls by name, and shadowing it
    would hand this hook a JSON string where it expects a list.
    """
    element_result = _element_interaction(obj, manipulator, action, element, values)
    if element_result is not None:
        return element_result
    if action != CREATE:
        return None
    builder = _BUILDERS.get(manipulator)
    if builder is None:
        return None
    name, shape_of, expected = builder
    return _commit(name, shape_of, [float(value) for value in values], expected)


FerroCAD.register_interaction_provider(_interaction)


class _ElementCommand(_DrawCommand):
    """A command that arms one element verb for the next pick."""

    VERB = OFFSET

    def Activated(self, iMsg=0):
        return _arm(self.VERB)


class _OffsetCommand(_ElementCommand):
    """``SimpleCAD_Offset`` — offset a face or a drawing edge."""

    VERB = OFFSET
    MENU = "Offset"
    TIP = "Pick a curved face or a drawing edge, then drag (or type) the distance"


class _ElongateCommand(_ElementCommand):
    """``SimpleCAD_Elongate`` — stretch a rim or a curved face into an ellipse."""

    VERB = ELONGATE
    MENU = "Elongate"
    TIP = "Pick a hole rim or a curved face, then drag (or type) the distance"


class _RotateElementCommand(_ElementCommand):
    """``SimpleCAD_Rotate`` — turn a drawing's own arc about its centre."""

    VERB = ROTATE
    MENU = "Rotate edge"
    TIP = "Pick a drawing's arc, then drag (or type) the angle"


for _name, _command in (
    ("Offset", _OffsetCommand()),
    ("Elongate", _ElongateCommand()),
    ("RotateEdge", _RotateElementCommand()),
):
    FreeCADGui.addCommand("SimpleCAD_" + _name, _command)
del _name, _command


def bootstrap(doc_name: str = "Sample") -> str:
    """Open the canonical sample and report host status.

    The edition seeds no demo of its own: the canvas-first workflow is drawing, and a
    seeded face would only be one more object to select around.
    """
    return _parametric_bootstrap(doc_name)
