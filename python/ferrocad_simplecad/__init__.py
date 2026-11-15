"""App-shell script for **FerroCAD: SimpleCAD**.

The edition is the Parametric composition plus the canvas-first tools: the same
shell data surface and the same Part display and interaction providers (a
SimpleCAD face *is* a ``Part::Feature``), with its own command set on top.
Re-exporting ``ferrocad_parametric`` rather than copying it keeps one definition
of "how a Part object is drawn and edited".

What this script adds:

* ``SimpleCAD_Rectangle`` — arm the native two-click rectangle. The tool is Rust
  (``ferrocad_stage::RectangleTool``), so it previews the box on the viewport and
  completes itself on the second press; this hook owns only what the commit *means*.
* the ``rectangle/create`` interaction: four world corners in, one ``Part::Feature``
  face out, in one named transaction, through SimpleCAD's own geometry
  (``simplecad.core.geometry.rectangle``) when that package is importable.

That split is the point of the edition: **the tool collects world geometry, the
script names the document object**. Every later SimpleCAD tool takes the same shape.

The shell imports this module by name (the edition's ``HostConfig.entry_module``),
so it must re-export every entry point the shell calls.
"""

from __future__ import annotations

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

#: The tool id the edition registers in its `Contributions` (see `src/main.rs`).
RECTANGLE_TOOL = "rectangle"
#: The action the tool's creating commit carries.
RECTANGLE_ACTION = "create"

#: The chrome surface this edition's commands sit on.
SURFACE = "SimpleCAD"


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


def _create_rectangle(values):
    """One `rectangle/create` commit: four corners to a `Part::Feature` face."""
    if len(values) < 12:
        return {"ok": False, "error": "a rectangle needs four corners"}
    corners = [FreeCAD.Vector(*values[index : index + 3]) for index in range(0, 12, 3)]
    document = FreeCAD.ActiveDocument
    if document is None:
        document = FreeCAD.newDocument("Unnamed")
    document.openTransaction("Rectangle")
    try:
        face = _face_from_corners(corners)
        feature = document.addObject("Part::Feature", "Rectangle")
        feature.Shape = face
        _add_appearance(feature)
        document.recompute()
        if _invalid(feature):
            document.abortTransaction()
            return {"ok": False, "error": "the rectangle could not be built"}
        document.commitTransaction()
    except Exception as exc:  # noqa: BLE001 - one bad commit must not break the shell
        document.abortTransaction()
        return {"ok": False, "error": str(exc)}
    return {"ok": True, "feature": feature.Name}


def _interaction(obj, manipulator, action, element, values):
    """Apply one of this edition's manipulator commits, or defer (`None`).

    Only ``rectangle/create`` is ours; every other manipulator belongs to the
    Parametric script (the dress-up scrubber, the push/pull stem, the sketch grips),
    whose provider runs beside this one.

    The name is underscored on purpose: ``interaction`` is the *shell* entry point
    (``ferrocad_shell.interaction``) the Python host calls by name, and shadowing it
    would hand this hook a JSON string where it expects a list.
    """
    if manipulator == RECTANGLE_TOOL and action == RECTANGLE_ACTION:
        return _create_rectangle([float(value) for value in values])
    return None


FerroCAD.register_interaction_provider(_interaction)


class _RectangleCommand:
    """``SimpleCAD_Rectangle`` — draw a rectangle in two clicks.

    The command only arms the tool; the drawing, the preview and the commit are the
    tool's, and `interaction` above turns the commit into an object.
    """

    def GetResources(self):
        return {
            "MenuText": "Rectangle",
            "ToolTip": "Click two opposite corners to draw a rectangle face",
            "Group": SURFACE,
            "CmdType": "ForEdit",
            "FerroCAD_Surface": SURFACE,
        }

    def IsActive(self):
        return True

    def Activated(self, iMsg=0):
        return {"tool": RECTANGLE_TOOL}


FreeCADGui.addCommand("SimpleCAD_Rectangle", _RectangleCommand())


def bootstrap(doc_name: str = "Sample") -> str:
    """Open the canonical sample and report host status.

    The edition seeds no demo of its own: the canvas-first workflow is drawing, and a
    seeded face would only be one more object to select around.
    """
    return _parametric_bootstrap(doc_name)
