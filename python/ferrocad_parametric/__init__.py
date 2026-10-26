"""App-shell script for **FerroCAD: Parametric**.

The base app's script (`ferrocad_shell`) is deliberately geometry-free: it knows the
document model and nothing about kernels. This edition links the Part workbench, so
it registers the two **module hooks** Part needs, seeds the demo solid, and offers
one creation command:

* `FerroCAD.register_display_provider` — how a `Part` object is tessellated for the
  viewport;
* `FerroCAD.register_interaction_provider` — what a manipulator commit means (the edge
  dress-up arc's `dress_up` / `fillet` / `chamfer`, the face push/pull stem's
  `push_pull` / `extrude` / `pocket`, and the generic gizmo's `transform` / `apply` as
  a placement change);
* `Part_Box` — a dock command that creates a box in one transaction, the way the toy
  workbench's `Toy_MakeSlab` does.

Objects this script creates carry `DisplayMode` and `Color` dynamic properties. A
`Part::Feature` has no view provider here, so the display hook reads those properties
and falls back to the edition's original opaque grey when they are absent (an opened
`.FCStd`, or an object created outside this workbench).

The Rust half of the same contribution (the scrubber and the stem) is
`ferrocad_part_gui::fillet_manipulator` and `ferrocad_part_gui::push_pull_manipulator`,
which the edition's `main` adds to `ferrocad::base_contributions()` — the composition
that already carries the shared translate gizmo. The shell hosts both through the
generic `Manipulator` and `TransformGizmo` seams, so it never names Part.

The shell imports this module by name (the edition's `HostConfig.entry_module`), so
it must re-export every entry point the shell calls.
"""

from __future__ import annotations

import math

import FreeCAD
import FreeCADGui
import FerroCAD
import FerroCADGui
import Part  # noqa: F401  (links the kernel; registers Part::Feature/Fillet/Chamfer)

# The shell's data functions (`bootstrap`, `model_tree`, `properties`,
# `set_property`, `evaluate`, `stage_meshes`, `interaction`, ...).
from ferrocad_shell import *  # noqa: F401,F403
from ferrocad_shell import bootstrap as _base_bootstrap

DEFAULT_LENGTH = 20.0
DEFAULT_WIDTH = 20.0
DEFAULT_HEIGHT = 20.0
# RGBA in the 0..1 range; alpha is unused. This is the opaque CAD grey the edition
# has always given Part objects, now also the default of their `Color` property.
DEFAULT_COLOR = (0.66, 0.72, 0.80, 1.0)

_DISPLAY_MODES = ("Shaded", "ShadedWithEdges", "Wireframe")
_DISPLAY_KEYS = {
    "Shaded": "shaded",
    "ShadedWithEdges": "shaded_edges",
    "Wireframe": "wireframe",
}


def _display_mesh(obj):
    """Tessellate a shape-bearing object (the Part display hook)."""
    # An invisible object is not drawn, the same as upstream's view provider. This is
    # what keeps a push/pull's intermediate `Part::Extrusion` out of the stage: the
    # boolean result is the visible solid, and the prism it was built from is hidden.
    try:
        if not obj.Visibility:
            return None
    except Exception:
        pass
    shape = getattr(obj, "Shape", None)
    if shape is None:
        return None
    try:
        if shape.isNull():
            return None
        mesh = shape.mesh()
        placement = getattr(obj, "Placement", None)
        _apply_object_placement(mesh, placement, shape=shape)
        # The object's own origin (the placement's base point). The triad hugs it — an
        # origin indicator — and a rotate pivot turns about it.
        if placement is not None:
            base = placement.Base
            mesh["origin"] = (base.x, base.y, base.z)
        # `DisplayMode` and `Color` are dynamic properties this workbench adds. An
        # opened `.FCStd` or a `Part::Feature` created elsewhere may not have them,
        # so fall back to the opaque grey surface with its edges.
        colors = getattr(obj, "Color", None)
        mesh["color"] = list(colors[0]) if colors else list(DEFAULT_COLOR)
        mode = str(getattr(obj, "DisplayMode", "") or "")
        mesh["display"] = _DISPLAY_KEYS.get(mode, "shaded_edges")
        return mesh
    except Exception:
        return None


def _apply_object_placement(mesh, placement, shape=None):
    """Move a tessellation from shape-local to world coordinates.

    ``Shape.mesh()`` is in the shape's own coordinates; a ``Part::Feature``'s
    ``Placement`` is a separate document property, the way FreeCAD's view provider
    applies it. The stage has no per-mesh placement field, so the display hook bakes
    it here. An opened ``.FCStd`` with placed objects therefore lands where it was
    saved, not at the origin.

    A ``PartShape``'s geometry is *always* baked with its own accumulated
    ``Placement`` (``geometry == placement · base``), and the ``Part::Feature``
    sync keeps that accumulated placement equal to the object's. Applying the
    object placement again would double it, so a shape whose placement already
    equals the object's is left alone. A Python proxy stores local geometry with
    an identity placement, so its object placement is applied here.
    """
    if placement is None:
        return
    if shape is not None:
        shape_placement = getattr(shape, "Placement", None)
        if shape_placement is not None:
            try:
                if shape_placement.isSame(placement, 1e-9):
                    return
            except Exception:
                pass
    positions = mesh.get("positions")
    if positions:
        mesh["positions"] = [
            tuple(placement.multVec(FreeCAD.Vector(*position)))
            for position in positions
        ]
    normals = mesh.get("normals")
    if normals:
        rotation = placement.Rotation
        mesh["normals"] = [
            tuple(rotation.multVec(FreeCAD.Vector(*normal))) for normal in normals
        ]


def _face_normal(obj, element):
    """The outward unit normal of a face, in the shape's **local** frame.

    The viewport's ``SelectionAnchor.normal`` is world-space because
    :func:`_apply_object_placement` bakes the object's ``Placement`` into the display
    tessellation. ``Part::Extrusion`` works in shape-local coordinates, so this hook
    re-derives the normal from the shape's own mesh rather than undoing the placement.

    The kernel duplicates vertices per face and tags each triangle with its part id, so
    the named face is exactly the triangles whose part id matches; their area-weighted
    cross-product sum is the outward normal. ``None`` when the element is absent or the
    triangles cancel (a non-planar face).
    """
    shape = getattr(obj, "Shape", None)
    if shape is None or shape.isNull():
        return None
    mesh = shape.mesh()
    parts = list(mesh.get("parts") or ())
    try:
        face = parts.index(element)
    except ValueError:
        return None
    positions = mesh.get("positions") or ()
    triangles = mesh.get("triangles") or ()
    triangle_parts = mesh.get("triangle_parts") or ()
    normal = [0.0, 0.0, 0.0]
    for index, part in enumerate(triangle_parts):
        if part != face:
            continue
        base = 3 * index
        try:
            a = positions[triangles[base]]
            b = positions[triangles[base + 1]]
            c = positions[triangles[base + 2]]
        except (IndexError, TypeError):
            continue
        ab = (b[0] - a[0], b[1] - a[1], b[2] - a[2])
        ac = (c[0] - a[0], c[1] - a[1], c[2] - a[2])
        normal[0] += ab[1] * ac[2] - ab[2] * ac[1]
        normal[1] += ab[2] * ac[0] - ab[0] * ac[2]
        normal[2] += ab[0] * ac[1] - ab[1] * ac[0]
    length = math.sqrt(sum(component * component for component in normal))
    if length < 1e-12:
        return None
    return tuple(component / length for component in normal)


def _push_pull(obj, element, distance):
    """Extrude or pocket the named face by ``distance`` in one transaction.

    ``distance`` is signed along the face's outward normal. The extrusion is built in
    the body's **shape-local** frame (``_face_normal``) and a ``Part::Extrusion``
    resolves the named sub-shape as its ``Base``. A positive distance is fused, a
    negative one cut; both are one undoable transaction. The intermediate extrusion is
    hidden, so the boolean result is the only solid the display hook draws.
    """
    if abs(distance) < 1e-9:
        return {"ok": True}
    if not element:
        return {"ok": False, "error": "push/pull needs a face element"}
    normal = _face_normal(obj, element)
    if normal is None:
        return {"ok": False, "error": f"{element} is not a planar face"}

    doc = obj.Document
    label = "Extrude" if distance > 0.0 else "Pocket"
    doc.openTransaction(f"{label} {obj.Name}")
    try:
        extrusion = doc.getObject(f"{obj.Name}_Extrusion")
        if extrusion is None:
            extrusion = doc.addObject("Part::Extrusion", f"{obj.Name}_Extrusion")
        extrusion.Base = (obj, [element])
        extrusion.DirMode = "Custom"
        extrusion.Dir = FreeCAD.Vector(*normal)
        extrusion.Solid = True
        extrusion.LengthFwd = abs(distance)
        extrusion.Reversed = distance < 0.0
        extrusion.Visibility = False

        kind = "Part::Fuse" if distance > 0.0 else "Part::Cut"
        feature_name = f"{obj.Name}_{'Fuse' if distance > 0.0 else 'Cut'}"
        feature = doc.getObject(feature_name)
        if feature is None:
            feature = doc.addObject(kind, feature_name)
        feature.Base = obj
        feature.Tool = extrusion
        # The boolean is built from shape-local operands (the display hook bakes the
        # body's `Placement`), and `Part::Fuse`/`Cut` carry no Placement of their own,
        # so give the result the body's placement to keep a moved body where it was.
        placement = getattr(obj, "Placement", None)
        if placement is not None:
            if not hasattr(feature, "Placement"):
                feature.addProperty(
                    "App::PropertyPlacement",
                    "Placement",
                    "Base",
                    "Placement of the combined solid",
                )
            feature.Placement = placement
        doc.recompute()
        doc.commitTransaction()
    except Exception as exc:
        doc.abortTransaction()
        return {"ok": False, "error": str(exc)}
    return {"ok": True, "feature": feature_name}


def _interaction(obj, manipulator, action, element, values):
    """Apply a manipulator commit: a Part dress-up, a push/pull or a move.

    Three operations reach this workbench:

    * ``dress_up`` / ``fillet``/``chamfer`` — the Rust edge scrubber; ``values`` are
      the radius and the second distance.
    * ``push_pull`` / ``extrude``/``pocket`` — the Rust face stem; ``values[0]`` is the
      signed distance along the face normal. Outward is an ``extrude`` (fuse), inward a
      ``pocket`` (cut).
    * ``transform`` / ``apply`` — the generic ``TransformGizmo``; ``values`` are the 16
      row-major elements of the world-space transform, composed with the object's
      current placement (the same contract the toy workbench implements).

    The manipulator names its operation and orders the numbers; this hook owns what
    that means for Part. The feature is named ``<object>_<mode>`` and reused on later
    scrubs. The edit is one named transaction (a single undo step), and the document
    is recomputed before returning, so the caller can re-read :func:`stage_meshes`
    straight away.
    """
    if manipulator == "transform" and action == "apply" and len(values) == 16:
        delta = FreeCAD.Placement(FreeCAD.Matrix(*values))
        doc = obj.Document
        doc.openTransaction(f"Move {obj.Name}")
        try:
            obj.Placement = delta * obj.Placement
            doc.recompute()
            doc.commitTransaction()
        except Exception as exc:
            doc.abortTransaction()
            return {"ok": False, "error": str(exc)}
        return {"ok": True}

    if manipulator == "push_pull" and action in ("extrude", "pocket") and values:
        return _push_pull(obj, element, float(values[0]))

    if manipulator != "dress_up":
        return None
    mode = {"fillet": "Fillet", "chamfer": "Chamfer"}.get(action)
    if mode is None:
        return None
    edges = [element] if element else []
    first = float(values[0]) if values else 0.0
    second = float(values[1]) if len(values) > 1 else 0.0

    doc = obj.Document
    feature_name = f"{obj.Name}_{mode}"
    doc.openTransaction(f"{mode} {obj.Name}")
    try:
        feature = doc.getObject(feature_name)
        if feature is None:
            feature = doc.addObject(f"Part::{mode}", feature_name)
        feature.Base = (obj, edges)
        if mode == "Fillet":
            feature.Radius = first
        else:
            feature.Distance = first
            feature.Distance2 = second
        doc.recompute()
        doc.commitTransaction()
    except Exception as exc:
        doc.abortTransaction()
        return {"ok": False, "error": str(exc)}
    return {"ok": True, "feature": feature_name}


FerroCAD.register_display_provider(_display_mesh)
FerroCAD.register_interaction_provider(_interaction)


#: The parametric edition's appearance preset (declared, not yet materialised).
#:
#: Parametric is the neutral edition: Part objects are the opaque grey
#: ``(0.66, 0.72, 0.80)`` (linear ``Y = 0.716``), so the gizmo has no blue object
#: to fight. The preset deliberately pins today's tokens rather than changing
#: them, so a reset restores the familiar look even if a shared default moves
#: later. Declaring the values also documents that the grey edition owns a
#: neutral palette, unlike the toy's contrast preset.
APPEARANCE_PRESETS = {
    "gizmo": {
        "active_color": "#38bdf8",
        "inactive_color": "#94a3b8",
    },
    "axis": {
        "color_x": "#f87171",
        "color_y": "#4ade80",
        "color_z": "#38bdf8",
    },
    "grid": {
        "color": "#94a3b8",
    },
}


def _register_appearance():
    """Declare the parametric preset through the GUI facade (idempotent)."""
    for feature, values in APPEARANCE_PRESETS.items():
        FerroCADGui.registerAppearancePreset(feature, values)


_register_appearance()


def _add_appearance(obj):
    """Add the display properties a bare ``Part::Feature`` does not define.

    Part objects have no view provider in this build, so the appearance is a pair of
    dynamic properties the display hook reads. The names and values match the toy
    workbench's ``DisplayMode``/``Color`` convention.
    """
    obj.addProperty(
        "App::PropertyEnumeration", "DisplayMode", "Part", "How the object is drawn"
    )
    obj.DisplayMode = list(_DISPLAY_MODES)
    obj.DisplayMode = "ShadedWithEdges"
    obj.addProperty("App::PropertyColor", "Color", "Part", "Surface colour")
    obj.Color = [DEFAULT_COLOR]
    return obj


def _add_box(doc, length, width, height, x, y, name):
    """Add the object and its properties; the caller owns the transaction."""
    box = doc.addObject("Part::Feature", name)
    box.Shape = Part.makeBox(float(length), float(width), float(height))
    _add_appearance(box)
    box.Placement = FreeCAD.Placement(FreeCAD.Vector(float(x), float(y), 0.0))
    return box


def makeBox(
    length=DEFAULT_LENGTH,
    width=DEFAULT_WIDTH,
    height=DEFAULT_HEIGHT,
    x=0.0,
    y=0.0,
    name="Box",
):
    """Create a Part box in the active document as one undoable transaction.

    This is the workbench's creation command: it adds a typed object with its
    properties, and the shell picks it up through the display hook on the next sync.
    """
    doc = FreeCAD.ActiveDocument
    if doc is None:
        doc = FreeCAD.newDocument("Unnamed")
    doc.openTransaction("Create Box")
    try:
        box = _add_box(doc, length, width, height, x, y, name)
        doc.recompute()
        doc.commitTransaction()
    except Exception:
        doc.abortTransaction()
        raise
    return box


class _MakeBoxCommand:
    """``Part_Box`` — add a box to the document."""

    def GetResources(self):
        return {
            "MenuText": "Create Box",
            "ToolTip": "Add a box to the document",
            "Group": "Part",
            "CmdType": "ForEdit",
            "FerroCAD_Surface": "dock",
        }

    def IsActive(self):
        return FreeCAD.ActiveDocument is not None

    def Activated(self):
        box = makeBox()
        return {"status": f"created {box.Name}"}


FreeCADGui.addCommand("Part_Box", _MakeBoxCommand())


def bootstrap(doc_name: str = "Shell") -> str:
    """The base sample document plus a 20 mm solid for the viewport."""
    # Materialise the edition preset under the user's persisted values. The
    # settings facades read that store, so every later command sees the preset.
    FerroCADGui.installAppearancePresets()
    payload = _base_bootstrap(doc_name)
    doc = FreeCAD.ActiveDocument
    _add_box(doc, DEFAULT_LENGTH, DEFAULT_WIDTH, DEFAULT_HEIGHT, 0.0, 0.0, "Box")
    doc.recompute()
    return payload
