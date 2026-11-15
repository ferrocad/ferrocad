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
  importable. The same commit **anchored on a planar face** imprints the profile into
  the host body instead (``simplecad.document.operations.draw_profile`` / ``draw_arc``),
  so the face divides into pickable sub-faces and no second object appears.

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
    from simplecad.modeling import slicing as _simplecad_slicing
    from simplecad.surfaces import edit as _simplecad_edit
    from simplecad.surfaces import offset as _simplecad_offset
except Exception:  # noqa: BLE001 - the edition works without the package
    _simplecad_geometry = None
    _simplecad_operations = None
    _simplecad_slicing = None
    _simplecad_edit = None
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


def _imprint(host, element, name, shape_of, values, expected):
    """Draw the committed profile **into** the face it was anchored on.

    The tool committed world geometry that already lies in the face's plane, so the
    modelling layer is the only judge of whether it fits: ``draw_profile`` (and
    ``draw_arc`` for an open arc) re-derives the support face, divides it, and opens
    **one** named transaction itself. No object is added and no view is rebuilt here:
    the host gains faces, which is exactly what the next Slot 2 push/pull needs.

    A profile that divides nothing is a refusal, not a silent success, and the
    document is untouched (the package writes nothing when the split finds no face).
    """
    if _simplecad_operations is None:
        return {
            "ok": False,
            "error": "imprinting needs the SimpleCAD modelling layer",
        }
    if len(values) < expected:
        return {"ok": False, "error": "%s needs %d numbers" % (name, expected)}
    try:
        host.Shape.getElement(element)
    except Exception as exc:  # noqa: BLE001 - an element the host cannot resolve
        return {"ok": False, "error": str(exc)}
    document = host.Document
    try:
        profile = shape_of(values)
        before = len(host.Shape.Faces)
        if name == "Arc":
            _simplecad_operations.draw_arc(document, profile, preferred=host)
        else:
            _simplecad_operations.draw_profile(document, profile, host, name)
        document.recompute()
        if len(host.Shape.Faces) <= before:
            raise ValueError("the profile did not divide %s" % element)
    except Exception as exc:  # noqa: BLE001 - one bad commit must not break the shell
        return {"ok": False, "error": str(exc)}
    return {"ok": True, "feature": host.Name}


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
    #: Which chrome surface owns the command: `creation` (the split pill), `radial`
    #: (a wheel wedge) or `palette` (Search only). It is never `dock`: the action dock
    #: holds the globals, the pill and Search, and nothing else.
    SURFACE_KIND = "creation"
    #: The creation pill's tier for a `creation` command.
    CATEGORY = "Sketch"
    #: The context kinds a `radial` command applies to.
    TARGET = ""
    #: `structural`, `additive` or `destructive` (the wheel's tone order).
    TONE = ""

    def GetResources(self):
        resources = {
            "MenuText": self.MENU,
            "ToolTip": self.TIP,
            "Group": SURFACE,
            "CmdType": "ForEdit",
            "FerroCAD_Surface": self.SURFACE_KIND,
        }
        if self.CATEGORY:
            resources["FerroCAD_Category"] = self.CATEGORY
        if self.TARGET:
            resources["FerroCAD_Target"] = self.TARGET
        if self.TONE:
            resources["FerroCAD_Tone"] = self.TONE
        return resources

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


def _interaction(obj, manipulator, action, element, values, host=None):
    """Apply one of this edition's commits, or defer (`None`).

    Three families are ours: the drawing tools' `create` commits (below), the element
    verbs' `set` commits (`_element_interaction`), and — when the tool anchored on a
    body's **face** — the imprint of that commit into the host (`_imprint`). Every
    other manipulator belongs to the Parametric script (the dress-up scrubber, the
    push/pull stem, the sketch grips), whose provider runs beside this one.

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
    numbers = [float(value) for value in values]
    host_object = (host or {}).get("object")
    host_element = str((host or {}).get("element") or "")
    if host_object is not None and host_element.startswith("Face"):
        return _imprint(host_object, host_element, name, shape_of, numbers, expected)
    # An edge host (or no host at all) creates a separate object: an outline drawn
    # along an edge is a profile of its own, and a split needs a face to split.
    return _commit(name, shape_of, numbers, expected)


FerroCAD.register_interaction_provider(_interaction)


class _ElementCommand(_DrawCommand):
    """A command that arms one element verb for the next pick.

    It shares `_DrawCommand`'s resource plumbing but is a **wheel wedge**, not a
    creation tool: the pick it follows is a curved face or a drawing's rim.
    """

    VERB = OFFSET
    SURFACE_KIND = "radial"
    CATEGORY = ""
    TARGET = "edge,face"
    TONE = "structural"

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
    TARGET = "edge"


for _name, _command in (
    ("Offset", _OffsetCommand()),
    ("Elongate", _ElongateCommand()),
    ("RotateEdge", _RotateElementCommand()),
):
    FreeCADGui.addCommand("SimpleCAD_" + _name, _command)
del _name, _command


# ---------------------------------------------------------------------------
# The selection verbs: one table, one gate, one transaction
#
# SimpleCAD's `SURFACE`, `Modify` and block operations act on a **selection** rather
# than on a pointer gesture. Each is wired here as `command -> gate -> pure function ->
# one transaction`:
#
# * the **gate** is the part of `command_state.REQUIREMENTS` that can be checked cheaply
#   — the object count and the element kind — and it refuses with a sentence that names
#   what to select, leaving the document untouched;
# * the **function** is the package's *pure* half (`surfaces/offset.py`,
#   `surfaces/edit.py`, `document/operations.py`), never the `surfaces.operations`
#   wrapper: those dispatch to a worker pool that an embedded interpreter cannot start
#   (the same reason `_offset_element` calls `offset_faces` directly);
# * the **transaction** is one undo step, opened either here (`_replace_shape`) or by the
#   document half itself (`commit_form`, `commit_slicing`, `unite`, `apply_plan`).
#
# The interface is deliberately dense: a selection plus one documented default number.
# The docked numeric row that would carry the number is C2 work (`ToolField`), so each
# verb names its value in the status line and `Activated(value)` accepts a better one —
# which is also how the round trip drives them.
# ---------------------------------------------------------------------------


class _Refusal(ValueError):
    """A selection the verb cannot serve; reported as the command's status."""


def _selection_pairs():
    """The live selection as ``(object, (element, …))`` pairs."""
    return [
        (item.Object, tuple(item.SubElementNames or ()))
        for item in FreeCADGui.Selection.getSelectionEx()
        if getattr(item, "Object", None) is not None
    ]


def _one_body(items, what, *, required=False):
    """``(object, [names])`` for a single body's selection."""
    if not items:
        raise _Refusal("select %s first" % what)
    if len({obj.Name for obj, _ in items}) != 1:
        raise _Refusal("select %s on one body" % what)
    obj, names = items[0]
    names = [name for _, elements in items for name in elements]
    if required and not names:
        raise _Refusal("select %s first" % what)
    return obj, names


def _indices(items, kind, what):
    """The 0-based indices of one element kind, refusing a mixed selection."""
    names = [name for _, elements in items for name in elements]
    other = "faces" if kind == "Face" else "edges"
    if any(not name.startswith(kind) for name in names):
        raise _Refusal("%s: select %s only" % (what, other))
    return sorted({int(name[len(kind):]) - 1 for name in names})


def _replace_shape(obj, shape, label):
    """Write ``shape`` onto ``obj`` in one transaction, refusing an invalid result."""
    if not shape.isValid():
        raise _Refusal("the result is not a valid shape; nothing was written")
    document = obj.Document
    document.openTransaction(label)
    try:
        obj.Shape = shape
        document.recompute()
        if _invalid(obj):
            raise _Refusal("the body refused the edit; nothing was written")
        document.commitTransaction()
    except Exception:  # noqa: BLE001 - one transaction, then re-raise
        document.abortTransaction()
        raise


def _new_shape(document, shape, label):
    """Add ``shape`` as its own one-transaction object and return it."""
    document.openTransaction(label)
    try:
        feature = document.addObject("Part::Feature", label.replace(" ", ""))
        feature.Label = label
        feature.Shape = shape
        _add_appearance(feature)
        document.recompute()
        if _invalid(feature):
            raise _Refusal("the result is not a valid shape; nothing was written")
        document.commitTransaction()
    except Exception:  # noqa: BLE001 - one transaction, then re-raise
        document.abortTransaction()
        raise
    return feature


def _verb_shell(thickness):
    items = _selection_pairs()
    obj, _names = _one_body(items, "faces to leave open")
    faces = _indices(items, "Face", "Shell")
    result, report = _simplecad_offset.shell(obj.Shape, faces, float(thickness))
    _replace_shape(obj, result, "Shell")
    return "shelled %s at %s mm (%d face(s) open)" % (
        obj.Name,
        thickness,
        report.get("open_faces", len(faces)),
    )


def _verb_draft(angle):
    items = _selection_pairs()
    obj, _names = _one_body(items, "the walls to draft", required=True)
    faces = _indices(items, "Face", "Draft")
    result, report = _simplecad_offset.draft(obj.Shape, faces, float(angle), (0, 0, 1))
    _replace_shape(obj, result, "Draft")
    return "drafted %d face(s) of %s by %s°" % (len(faces), obj.Name, angle)


def _verb_fill(_value=None):
    items = _selection_pairs()
    obj, _names = _one_body(items, "the rim edges to fill")
    edges = _indices(items, "Edge", "Fill hole")
    result, report = _simplecad_edit.fill(obj.Shape, edges or None)
    _replace_shape(obj, result, "Fill hole")
    return "filled %d opening(s) of %s" % (report.get("filled", 0), obj.Name)


def _verb_remove_rounds(max_radius):
    items = _selection_pairs()
    obj, names = _one_body(items, "the body to un-round")
    within = set(_indices(items, "Face", "Remove rounds")) if names else None
    found = _simplecad_edit.find_rounds(obj.Shape, float(max_radius), within=within)
    if not found:
        raise _Refusal(
            "no rounds up to %s mm here; raise the radius or pick the body" % max_radius
        )
    result, report = _simplecad_edit.remove_faces(
        obj.Shape, [round_info["index"] for round_info in found]
    )
    _replace_shape(obj, result, "Remove rounds")
    return "removed %d round(s) from %s" % (report.get("removed", len(found)), obj.Name)


def _verb_unwrap(_value=None):
    items = _selection_pairs()
    obj, _names = _one_body(items, "one face to unwrap", required=True)
    faces = _indices(items, "Face", "Unwrap")
    if len(faces) != 1:
        raise _Refusal("Unwrap: select exactly one face")
    flat, report = _simplecad_edit.unwrap(obj.Shape.Faces[faces[0]])
    feature = _new_shape(obj.Document, flat, "%sUnwrap" % obj.Name)
    return "unwrapped %s as %s (%s mm²)" % (
        obj.Name,
        feature.Label,
        round(float(report.get("area", 0.0)), 1),
    )


def _form(items, mode):
    """Loft or Sweep: the document half's prepare/form/commit trio."""
    if not items:
        raise _Refusal("select %s profile(s) first" % mode.lower())
    targets = _simplecad_operations.prepare_profiles(items, mode)
    result = _simplecad_operations.form_preview(targets, mode)
    feature = _simplecad_operations.commit_form(targets, mode, result)
    return "%s of %d profile(s) into %s" % (
        mode.lower(),
        len(targets),
        feature.Name,
    )


def _verb_loft(_value=None):
    return _form(_selection_pairs(), "Loft")


def _verb_sweep(_value=None):
    return _form(_selection_pairs(), "Sweep")


def _verb_union(_value=None):
    items = _selection_pairs()
    if any(elements for _, elements in items):
        raise _Refusal("Union: select whole objects, not faces or edges")
    objects = [obj for obj, _ in items]
    if len(objects) < 2:
        raise _Refusal("Union: select at least two objects")
    target = _simplecad_operations.unite(objects)
    return "united %d object(s) into %s" % (len(objects), target.Name)


def _verb_intersect(_value=None):
    items = _selection_pairs()
    if len(items) < 2:
        raise _Refusal("Intersect: select at least two faces or objects")
    _simplecad_operations.intersect(items)
    return "intersected %d selection(s)" % len(items)


def _verb_slice(offset):
    items = _selection_pairs()
    if any(elements for _, elements in items):
        raise _Refusal("Slice: select whole objects")
    if not items:
        raise _Refusal("Slice: select an object first")
    targets = _simplecad_operations.prepare_slicing(items)
    plane = _simplecad_slicing.cut_plane([obj.Shape for obj, _ in items], "Main")
    preview = _simplecad_operations.preview_slicing(targets, plane, float(offset))
    # The preview keeps `(side, shape)` pairs; the commit wants the shapes, optionally
    # spread apart. `separate` is the adapter the GUI tool uses for the same reason.
    planned = [
        (obj, _simplecad_slicing.separate(pieces, plane.normal, 0.0))
        for obj, pieces in preview
    ]
    pieces = _simplecad_operations.commit_slicing(targets, planned)
    return "sliced into %d piece(s)" % len(pieces)


def _verb_subdivide(divisions):
    items = _selection_pairs()
    obj, _names = _one_body(items, "one flat face to subdivide")
    faces = _indices(items, "Face", "Subdivide")
    target = _simplecad_operations.prepare_subdivision([(obj, ["Face%d" % (index + 1) for index in faces])])
    rows = columns = int(divisions)
    planned, regions = _simplecad_operations.preview_subdivision(target, rows, columns)
    count = len(regions) if hasattr(regions, "__len__") else rows * columns
    _simplecad_operations.commit_subdivision(target, planned, rows, columns, region_count=count)
    return "subdivided %s into %d region(s)" % (obj.Name, count)


def _verb_cleanup(_value=None):
    items = _selection_pairs()
    objects = [obj for obj, _ in items] or [
        obj for obj in FreeCAD.ActiveDocument.Objects if getattr(obj, "Shape", None) is not None
    ]
    if not objects:
        raise _Refusal("Cleanup: nothing to clean")
    _simplecad_operations.cleanup(FreeCAD.ActiveDocument, objects)
    return "cleaned %d object(s)" % len(objects)


def _verb_heal(_value=None):
    items = _selection_pairs()
    objects = [obj for obj, _ in items]
    if not objects:
        raise _Refusal("Heal: select the objects to heal")
    _simplecad_operations.heal(FreeCAD.ActiveDocument, objects)
    return "healed %d object(s)" % len(objects)


#: Verb -> (default number, commit, summary). The number is the value the command uses,
#: and what `Activated(value)` overrides; a verb with no number is `None`.
_VERBS = {
    "Shell": (1.0, _verb_shell, "Hollow the body, leaving the picked faces open"),
    "Draft": (5.0, _verb_draft, "Tilt the selected walls by an angle"),
    "FillHole": (None, _verb_fill, "Close the openings in a surface"),
    "RemoveRounds": (5.0, _verb_remove_rounds, "Remove rounds up to a radius"),
    "Unwrap": (None, _verb_unwrap, "Flatten one face beside the part"),
    "Loft": (None, _verb_loft, "Loft the selected profiles in order"),
    "Sweep": (None, _verb_sweep, "Sweep a profile along a path"),
    "Union": (None, _verb_union, "Join the selected objects into one solid"),
    "Intersect": (None, _verb_intersect, "Split the crossing faces or objects"),
    "Slice": (0.0, _verb_slice, "Slice the selected objects across an axis"),
    "Subdivide": (3, _verb_subdivide, "Divide one flat face into a grid"),
    "Cleanup": (None, _verb_cleanup, "Merge duplicate and touching geometry"),
    "Heal": (None, _verb_heal, "Align nearly parallel, coplanar or axis-aligned geometry"),
}

#: Verb -> (context kinds, tone) for the wheel. The ring is a **context** menu: a
#: shell verb belongs on a face, a dress-up on an edge, and a combine verb on a body.
#: `Delete` is the shell's own destructive slot and is always last.
_VERB_WHEEL = {
    "Shell": ("body,face", "structural"),
    "Draft": ("face", "structural"),
    "FillHole": ("face", "structural"),
    "RemoveRounds": ("edge,face", "structural"),
    "Unwrap": ("face", "structural"),
    "Union": ("body", "additive"),
    "Intersect": ("body", "additive"),
    "Subdivide": ("face", "structural"),
}

#: Verbs with no context gesture to attach to: Search lists them, the wheel does not.
#: A slice needs a plane and a heal needs a chosen body, so neither is a flick.
_PALETTE_VERBS = {"Slice", "Cleanup", "Heal"}

#: Verbs that **arm a shape** rather than act on the current pick. They are creation-pill
#: tools, so they are reached the same way `Rectangle` is — from `⊕ <tool> ▾`.
_CREATION_VERBS = {"Loft": "Surface", "Sweep": "Surface"}

#: Verb -> the topology it needs before the chrome offers it (`FerroCAD_Gate`). The
#: pill's rows and the wheel read this, so an impossible verb is greyed with its
#: requirement rather than silently doing nothing.
_VERB_GATE = {
    "Shell": "face:1",
    "Draft": "face:1",
    "FillHole": "face:1",
    "Unwrap": "face:1",
    "RemoveRounds": "edge:1",
    "Subdivide": "face:1:1",
    "Union": "body:2",
    "Intersect": "body:2",
    "Loft": "body:2",
    "Sweep": "body:2",
}


def _run_verb(verb, value=None):
    """Run one selection verb. A refusal is a status, never an exception."""
    spec = _VERBS.get(verb)
    if spec is None:
        return {"ok": False, "status": "no such verb %s" % verb}
    default, commit, _summary = spec
    if _simplecad_operations is None or _simplecad_edit is None or _simplecad_offset is None:
        return {
            "ok": False,
            "status": "%s needs the SimpleCAD package on the path" % verb.lower(),
        }
    try:
        return {"ok": True, "status": commit(default if value is None else value)}
    except Exception as exc:  # noqa: BLE001 - one refusal must not break the shell
        return {"ok": False, "status": str(exc) or "%s failed" % verb.lower()}


class _VerbCommand:
    """A ``SimpleCAD_*`` command that runs one selection verb on the current selection."""

    def __init__(self, verb):
        self.verb = verb
        self.default, _commit, self.summary = _VERBS[verb]

    def GetResources(self):
        tooltip = self.summary
        if self.default is not None:
            tooltip += " (%s by default)" % self.default
        resources = {
            "MenuText": self.verb,
            "ToolTip": tooltip,
            "Group": SURFACE,
            "CmdType": "ForEdit",
            "FerroCAD_Surface": "radial",
        }
        if self.verb in _CREATION_VERBS:
            resources["FerroCAD_Surface"] = "creation"
            resources["FerroCAD_Category"] = _CREATION_VERBS[self.verb]
        elif self.verb in _PALETTE_VERBS:
            resources["FerroCAD_Surface"] = "palette"
        else:
            target, tone = _VERB_WHEEL.get(self.verb, ("body", "structural"))
            resources["FerroCAD_Target"] = target
            resources["FerroCAD_Tone"] = tone
        gate = _VERB_GATE.get(self.verb)
        if gate:
            resources["FerroCAD_Gate"] = gate
        return resources

    def IsActive(self):
        return FreeCAD.ActiveDocument is not None and bool(_selection_pairs())

    def Activated(self, value=None):
        return _run_verb(self.verb, value)


for _verb in _VERBS:
    FreeCADGui.addCommand("SimpleCAD_" + _verb, _VerbCommand(_verb))
del _verb


def bootstrap(doc_name: str = "Sample") -> str:
    """Open the canonical sample and report host status.

    The edition seeds no demo of its own: the canvas-first workflow is drawing, and a
    seeded face would only be one more object to select around.
    """
    return _parametric_bootstrap(doc_name)
