"""A minimal, pure-Python ``Draft`` compatibility shim.

FerroCAD does **not** ship the Draft workbench. The shim exists for two
operations: ``make_clone`` (upstream Part's ``parttests/TestPartMirror.py``
does ``import Draft`` and calls it, and ``TestPartApp.py`` imports that test
file at module scope) and ``make_shape_string`` / ``make_shapestring`` (the
scripted Draft ShapeString, which turns a font into wires or faces).

What this module implements
---------------------------

``make_clone(source, delta=None, forcedraft=False)``
    Add an ``App::Link`` to the active document, point its ``LinkedObject`` at
    ``source``, and start it at the source's ``Placement`` (matching upstream
    ``Draft.make_clone``). ``delta`` offsets the clone's ``Placement.Base``.

    FerroCAD's ``App::Link`` is the existing object type that already means
    "live copy of another object's ``Shape``, transformed by a ``Placement``":
    its ``execute`` reads the linked object's ``Shape`` and bakes its own
    ``Placement`` into the derived ``Shape`` on every recompute. That is
    exactly the behaviour the mirror test needs, so this shim reuses it
    instead of introducing a private object type. ``make_clone`` recomputes
    afterwards so the returned object already has a ``Shape``.

What this module does **not** implement
---------------------------------------

Everything else in Draft: no GUI, no editing tools, no importers, no
2D-clone special-casing, no Arch/BIM clone modes (``forcedraft`` is accepted
only for signature compatibility). A name outside ``__all__`` raises
``AttributeError`` naming this module and the missing workbench surface; it is
never silently replaced by a placeholder. The full workbench remains upstream
Python, shadowed here only for the headless Part tests.

``make_shape_string(string, fontfile, size=100, tracking=0)``
    Add a ``Part::Part2DObjectPython`` whose proxy rebuilds the text as a
    compound of wires (or faces, when ``MakeFace`` is on) on every recompute.
    This is upstream's ``Draft ShapeString`` object in its scripted form: the
    geometry comes from ``Part.makeWireString``, the properties are the ones
    ``execute`` reads (``String``, ``FontFile``, ``Size``, ``Tracking``,
    ``MakeFace``, ``ScaleToSize``, ``Justification``, ``JustificationReference``,
    ``KeepLeftMargin``, ``ObliqueAngle``). ``make_shapestring`` and
    ``makeShapeString`` are the deprecated upstream aliases.

    Not implemented here, and not declared as properties: ``Fuse`` (upstream
    fuses and concatenates the faces, which needs Draft's ``faces.concatenate``)
    and the sticky-font probe that disables ``MakeFace`` for fonts whose
    contours it cannot fill. Both are recorded in
    ``docs/project/tasks/text-and-3d-text.md``.

Notes on the ``App::Link`` reuse
--------------------------------

* ``Shape`` is an *output* property of ``App::Link`` (derived by ``execute``,
  not writable), so ``make_clone`` recomputes before returning rather than
  assigning ``Shape``.
* ``App::Link`` applies its ``Placement`` to the linked object's ``Shape``.
  Like upstream ``Draft.make_clone`` the clone therefore starts at the
  source's ``Placement``. Cloning an object whose ``Shape`` is already in
  world coordinates (another ``App::Link``, for example) would compose that
  placement twice; the Part tests do not do this.
"""

from __future__ import annotations

import math
import os

import FreeCAD as App

__all__ = [
    "make_clone",
    "make_shape_string",
    "make_shapestring",
    "makeShapeString",
    "ShapeString",
]

#: The font types upstream accepts for a ShapeString.
_FONT_TYPES = (".ttc", ".ttf", ".otf", ".pfb")

#: Upstream's justification options, in its order.
_JUSTIFICATIONS = [
    "Top-Left",
    "Top-Center",
    "Top-Right",
    "Middle-Left",
    "Middle-Center",
    "Middle-Right",
    "Bottom-Left",
    "Bottom-Center",
    "Bottom-Right",
]

#: Upstream's height references for justification.
_REFERENCES = ["Cap Height", "Shape Height"]

_CLONE_TYPE = "App::Link"
#: The default name upstream's ``Draft.make_clone`` gives the new object.
_CLONE_NAME = "Clone"


def make_clone(source, delta=None, forcedraft=False):
    """Clone ``source`` into the active document as an ``App::Link``.

    Parameters
    ----------
    source :
        A document object. A one-element list/tuple is accepted, as upstream
        accepts ``clone(obj)`` and ``clone([obj])``. Cloning several objects
        at once is not implemented here and raises ``NotImplementedError``.
    delta :
        Optional ``Base.Vector`` added to the clone's ``Placement.Base``.
    forcedraft :
        Accepted for signature compatibility with upstream and ignored; there
        are no Arch/BIM clone modes in FerroCAD.

    Returns
    -------
    The new ``App::Link`` document object, which owns a ``Shape`` derived from
    ``source`` and a settable ``Placement``. A later ``doc.recompute()`` keeps
    ``Shape`` consistent with ``Placement``.
    """
    del forcedraft  # no Arch/BIM clone modes to force Draft over

    if isinstance(source, (list, tuple)):
        if len(source) != 1:
            raise NotImplementedError(
                "Draft.make_clone clones exactly one object in FerroCAD; "
                f"got {len(source)} objects"
            )
        source = source[0]

    doc = App.ActiveDocument
    if doc is None:
        raise RuntimeError(
            "Draft.make_clone needs an active document; "
            "call FreeCAD.newDocument() first"
        )

    clone = doc.addObject(_CLONE_TYPE, _CLONE_NAME)
    clone.LinkedObject = source

    # Match upstream: the clone starts at the source's placement.
    placement = getattr(source, "Placement", None)
    if placement is not None:
        clone.Placement = placement
    if delta is not None:
        # ``Base`` is a live view of the placement property; assigning the
        # summed vector writes the moved placement back. This also works when
        # the source has no ``Placement``, where the clone starts at identity.
        clone.Placement.Base = clone.Placement.Base + delta

    # ``App::Link.Shape`` is an output property, so execute it once now: the
    # returned object then really has the copied shape, not an empty output.
    doc.recompute()
    return clone


def make_shape_string(string, fontfile, size=100, tracking=0):
    """Add a ``Draft ShapeString`` object to the active document.

    Parameters
    ----------
    string : str
        The text. One line only, left to right (as upstream).
    fontfile : str
        Path to a ``.ttf``/``.otf``/``.ttc``/``.pfb`` font. A path starting with
        ``.`` is resolved against the document's directory (upstream 1.1).
    size : float
        The height in millimetres. With ``ScaleToSize`` on (the default) this is
        the cap height, not the font's line height.
    tracking : float
        Inter-character spacing in millimetres.

    Returns
    -------
    The new object, already recomputed. Its ``Shape`` is a compound of faces
    when ``MakeFace`` is on, else a compound of wires.
    """
    doc = App.ActiveDocument
    if doc is None:
        raise RuntimeError(
            "Draft.make_shape_string needs an active document; "
            "call FreeCAD.newDocument() first"
        )
    obj = doc.addObject("Part::Part2DObjectPython", "ShapeString")
    ShapeString(obj)
    obj.String = string
    obj.FontFile = fontfile
    obj.Size = size
    obj.Tracking = tracking
    doc.recompute()
    return obj


def make_shapestring(String, FontFile, Size=100, Tracking=0):
    """Deprecated alias of :func:`make_shape_string` (upstream 26.3)."""
    return make_shape_string(String, FontFile, size=Size, tracking=Tracking)


def makeShapeString(String, FontFile, Size=100, Tracking=0):
    """Deprecated alias of :func:`make_shape_string` (upstream 26.3)."""
    return make_shapestring(String, FontFile, Size=Size, Tracking=Tracking)


class ShapeString:
    """The ``Draft ShapeString`` proxy, in the form a script can build.

    The geometry is rebuilt in ``execute`` from ``String``, ``FontFile``,
    ``Size`` and ``Tracking``; ``MakeFace`` chooses faces (which extrude to
    solids) over bare wires (which extrude to shells). ``ScaleToSize`` makes
    ``Size`` the cap height, ``Justification``/``JustificationReference`` place
    the text, and ``ObliqueAngle`` applies upstream's shear.
    """

    def __init__(self, obj):
        obj.Proxy = self
        self.add_properties(obj)

    def add_properties(self, obj):
        existing = obj.PropertiesList

        def add(type_name, name, description):
            if name not in existing:
                obj.addProperty(type_name, name, "Draft", description)

        add("App::PropertyString", "String", "Text string")
        add("App::PropertyFile", "FontFile", "Font file name")
        add("App::PropertyLength", "Size", "Height of text")
        add("App::PropertyDistance", "Tracking", "Inter-character spacing")
        add("App::PropertyBool", "MakeFace", "Fill letters with faces")
        add(
            "App::PropertyBool",
            "ScaleToSize",
            "Scale to ensure cap height is equal to size",
        )
        add(
            "App::PropertyBool",
            "KeepLeftMargin",
            "Keep left margin and leading white space when justification is left",
        )
        add("App::PropertyAngle", "ObliqueAngle", "Oblique (slant) angle")
        add(
            "App::PropertyEnumeration",
            "Justification",
            "Horizontal and vertical alignment",
        )
        add(
            "App::PropertyEnumeration",
            "JustificationReference",
            "Height reference used for justification",
        )

        if "Justification" not in existing:
            obj.Justification = list(_JUSTIFICATIONS)
            obj.Justification = "Bottom-Left"
        if "JustificationReference" not in existing:
            obj.JustificationReference = list(_REFERENCES)
            obj.JustificationReference = "Cap Height"
        if "Size" not in existing:
            obj.Size = 100.0
        if "Tracking" not in existing:
            obj.Tracking = 0.0
        if "MakeFace" not in existing:
            obj.MakeFace = True
        if "ScaleToSize" not in existing:
            obj.ScaleToSize = True
        if "KeepLeftMargin" not in existing:
            obj.KeepLeftMargin = False
        if "ObliqueAngle" not in existing:
            obj.ObliqueAngle = 0.0
        if "String" not in existing:
            obj.String = ""
        if "FontFile" not in existing:
            obj.FontFile = ""

    def execute(self, obj):
        import Part

        font = self._font_path(obj)
        if not obj.String or not font:
            obj.Shape = Part.Compound([])
            return
        if not os.path.isfile(font):
            App.Console.PrintError(
                f"{obj.Label}: ShapeString font file not found: {font}\n"
            )
            obj.Shape = Part.Compound([])
            return
        if os.path.splitext(font)[1].lower() not in _FONT_TYPES:
            App.Console.PrintError(
                f"{obj.Label}: ShapeString font type is not supported: {font}\n"
            )
            obj.Shape = Part.Compound([])
            return

        size = float(obj.Size)
        tracking = float(obj.Tracking)
        shapes = []
        for char in Part.makeWireString(obj.String, font, size, tracking):
            if not char:
                continue
            if obj.MakeFace:
                shapes.extend(self._faces(char, Part))
            else:
                shapes.extend(char)
        if not shapes:
            App.Console.PrintWarning(
                f"{obj.Label}: ShapeString string has no wires\n"
            )
            obj.Shape = Part.Compound([])
            return

        shape = Part.Compound(shapes)
        cap_char = Part.makeWireString("M", font, size, tracking)[0]
        cap_height = Part.Compound(cap_char).BoundBox.YMax if cap_char else 0.0
        if obj.ScaleToSize and cap_height > 0:
            shape.scale(size / cap_height)
            cap_height = size

        angle = float(obj.ObliqueAngle)
        if angle:
            if -80 <= angle <= 80:
                matrix = App.Matrix()
                matrix.A12 = math.tan(math.radians(angle))
                shape = shape.transformGeometry(matrix)
            else:
                App.Console.PrintWarning(
                    "ShapeString: oblique angle must be in the -80 to +80 degree range\n"
                )

        shape.translate(self._justification(shape, cap_height, obj))
        obj.Shape = shape

    @staticmethod
    def _font_path(obj):
        font = obj.FontFile
        if not font:
            return ""
        if font.startswith("."):
            document = getattr(obj.Document, "FileName", "") or ""
            return os.path.abspath(os.path.join(os.path.dirname(document), font))
        return font

    @staticmethod
    def _faces(char, part):
        wires = [wire for wire in char if wire.isClosed()]
        if not wires:
            return []
        for maker in (
            "Part::FaceMakerBullseye",
            "Part::FaceMakerCheese",
            "Part::FaceMakerSimple",
            "Part::FaceMakerUnified",
        ):
            try:
                faces = part.makeFace(wires, maker).Faces
                for face in faces:
                    face.validate()
                return faces
            except (part.OCCError, IndexError, ValueError):
                continue
        App.Console.PrintWarning(
            "ShapeString: face creation failed for one character\n"
        )
        return []

    @staticmethod
    def _justification(shape, cap_height, obj):
        box = shape.optimalBoundingBox()
        if obj.KeepLeftMargin and "Left" in obj.Justification:
            vector = App.Vector(0, 0, 0)
        else:
            # Drop the left side bearing, as upstream does.
            vector = App.Vector(-box.XMin, 0, 0)
        width = box.XLength
        if "Shape" in obj.JustificationReference:
            vector = vector + App.Vector(0, -box.YMin, 0)
            height = box.YLength
        else:
            height = cap_height
        if "Top" in obj.Justification:
            vector = vector + App.Vector(0, -height, 0)
        elif "Middle" in obj.Justification:
            vector = vector + App.Vector(0, -height / 2, 0)
        if "Right" in obj.Justification:
            vector = vector + App.Vector(-width, 0, 0)
        elif "Center" in obj.Justification:
            vector = vector + App.Vector(-width / 2, 0, 0)
        return vector

    def dumps(self):
        """Nothing to persist: every input is a property."""
        return None

    def loads(self, state):
        del state


def __getattr__(name):
    raise AttributeError(
        f"module {__name__!r} is a compatibility shim, not the Draft "
        f"workbench; it implements only {{{', '.join(__all__)}}}"
    )
