"""App-shell script for **FerroCAD: Parametric**.

The base app's script (`ferrocad_shell`) is deliberately geometry-free: it knows the
document model and nothing about kernels. This edition links the Part workbench, so
it registers the two **module hooks** Part needs, seeds the demo solid, and offers
these commands:

* `FerroCAD.register_display_provider` — how a `Part` object is tessellated for the
  viewport;
* `FerroCAD.register_interaction_provider` — what a manipulator commit means (the edge
  dress-up arc's `dress_up` / `fillet` / `chamfer`, the face push/pull stem's
  `push_pull` / `extrude` / `pocket`, and the generic gizmo's `transform` / `apply` as
  a placement change);
* `Part_Box` — a dock command that creates a box in one transaction, the way the toy
  workbench's `Toy_MakeSlab` does;
* `Part_Polyline` — the polyline tool (a native `polyline` session from the edition's
  ``Contributions``) and the creating commit it finishes with: one transaction makes a
  parametric `Part::FeaturePython` wire and the `Part::Extrusion` that links it, so
  editing the wire's points moves the pad;
* `Part_Fuse`, `Part_Cut`, `Part_Common` and `Part_Compound` — the selection-driven
  boolean and compound commands: one object per run, the operands hidden behind a
  boolean, and the shape work left to the object types;
* `App_SaveDatumPlane` — a view command that saves the shell's live workplane as a
  persistent `Part::DatumPlane`, so it can be picked, moved, hidden, saved and
  bound with `Set WP`.

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

import json
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
#: The extrusion height a finished polyline gets (mm); editable on the wire's
#: ``Thickness`` property.
DEFAULT_WIRE_THICKNESS = 10.0
# RGBA in the 0..1 range; alpha is unused. This is the opaque CAD grey the edition
# has always given Part objects, now also the default of their `Color` property.
#
# The renderer treats this as **linear** albedo and writes to an sRGB target, so a
# light linear value encodes to near-white on screen. The old (0.66, 0.72, 0.80)
# was 0.87 sRGB once lit; this mid tone stays a readable steel grey.
DEFAULT_COLOR = (0.30, 0.34, 0.40, 1.0)

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
        # The stored geometry is already in world coordinates: FreeCAD applies a
        # `Part::Feature` placement when the shape is assigned (and this tree re-bakes
        # it in the feature sync), so the positions are used as given. Applying the
        # object placement again would move every object a second time.
        # `Part.DISPLAY_DEFLECTION`, not `Shape.mesh`'s own default: the memo behind
        # the mesh holds one entry per shape, and the host meshes the document at this
        # deflection — different numbers would evict each other's entry and re-mesh.
        mesh = shape.mesh(deflection=Part.DISPLAY_DEFLECTION, binary=True)
        placement = getattr(obj, "Placement", None)
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
        # A datum plane is a bounded translucent card, not a solid face. `glass`
        # is the display hook's opt-in (the host's DisplaySync picks the blended
        # pass); every other Part object keeps its opaque surface.
        if str(getattr(obj, "TypeId", "") or "") == "Part::DatumPlane":
            mesh["display"] = "glass"
        return mesh
    except Exception:
        return None


def _face_normal(obj, element):
    """The outward unit normal of a face, in the shape's **local** frame.

    The viewport's ``SelectionAnchor.normal`` is world-space, and so is the shape's own
    mesh (the placement is baked when the shape is assigned). ``Part::Extrusion`` works
    in the same coordinates, so the normal is read straight off the shape's mesh.

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

    Picking the cap a previous pull produced **edits that pull** instead of stacking a
    second feature over the same body; :func:`_extrusion_target` is how the geometry
    says which feature the face belongs to.
    """
    if abs(distance) < 1e-9:
        return {"ok": True}
    if not element:
        return {"ok": False, "error": "push/pull needs a face element"}

    existing = _extrusion_target(obj, element)
    if existing is not None:
        return _edit_extrusion(obj, existing, distance)

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
        if _invalid(feature):
            doc.abortTransaction()
            return {
                "ok": False,
                "error": f"{label.lower()} on {element} failed; the body is unchanged",
            }
        doc.commitTransaction()
    except Exception as exc:
        doc.abortTransaction()
        return {"ok": False, "error": str(exc)}
    return {"ok": True, "feature": feature_name}


def _invalid(object_):
    """Whether a feature's last rebuild failed.

    A Part feature whose kernel operation refuses the input is marked invalid and keeps
    its previous `Shape` (or none on a first build). Committing that state is how a solid
    disappears from the viewport, so every edit checks it and aborts instead.
    """
    try:
        return "Invalid" in object_.State
    except Exception:  # noqa: BLE001 - an object without State is assumed valid
        return False


def _edit_extrusion(obj, target, distance):
    """Change the extrusion a cap face belongs to, as one transaction.

    ``obj`` is the boolean the cap is drawn on and ``target`` is what
    :func:`_extrusion_target` resolved. The boolean is reused while the sign — and so
    the operation — stays the same; crossing zero swaps a fuse for a cut. The extrusion
    object is reused either way, so a body still has one push/pull feature.
    """
    extrusion, base_obj, base_element, current = target
    if abs(distance - current) < 1e-9:
        return {"ok": True, "feature": obj.Name}

    doc = obj.Document
    label = "Extrude" if distance > 0.0 else "Pocket"
    kind = "Part::Fuse" if distance > 0.0 else "Part::Cut"
    feature_name = f"{base_obj.Name}_{'Fuse' if distance > 0.0 else 'Cut'}"
    doc.openTransaction(f"{label} {base_obj.Name}")
    try:
        extrusion.Base = (base_obj, [base_element])
        extrusion.Solid = True
        extrusion.LengthFwd = abs(distance)
        extrusion.Reversed = distance < 0.0
        extrusion.Visibility = False

        feature = doc.getObject(feature_name)
        if feature is None:
            feature = doc.addObject(kind, feature_name)
        feature.Base = base_obj
        feature.Tool = extrusion
        placement = getattr(obj, "Placement", None) or getattr(
            base_obj, "Placement", None
        )
        if placement is not None:
            if not hasattr(feature, "Placement"):
                feature.addProperty(
                    "App::PropertyPlacement",
                    "Placement",
                    "Base",
                    "Placement of the combined solid",
                )
            feature.Placement = placement
        # The cap's old boolean is retired when the operation changed; the extrusion it
        # pointed at is the one that just moved.
        if obj.Name != feature_name:
            doc.removeObject(obj.Name)
        doc.recompute()
        if _invalid(feature):
            doc.abortTransaction()
            return {
                "ok": False,
                "error": f"{label.lower()} edit failed; the body is unchanged",
            }
        doc.commitTransaction()
    except Exception as exc:
        doc.abortTransaction()
        return {"ok": False, "error": str(exc)}
    return {"ok": True, "feature": feature_name}


def _edit_dress_up(obj, mode, first, second):
    """Change an existing ``Part::Fillet``/``Chamfer`` in place, as one transaction.

    The picked face *is* the feature's own result, so the feature is the edit target:
    the base link and the edge selection stay as they are and only the value moves.
    """
    doc = obj.Document
    doc.openTransaction(f"{mode} {obj.Name}")
    try:
        if mode == "Fillet":
            obj.Radius = first
        else:
            obj.Distance = first
            obj.Distance2 = second
        doc.recompute()
        if _invalid(obj):
            doc.abortTransaction()
            return {"ok": False, "error": f"{mode.lower()} edit failed; the body is unchanged"}
        doc.commitTransaction()
    except Exception as exc:
        doc.abortTransaction()
        return {"ok": False, "error": str(exc)}
    return {"ok": True, "feature": obj.Name}


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
    if manipulator == "polyline" and action == "create":
        # A creating commit: there is no target object, the points are the input.
        return _create_polyline([float(value) for value in values])

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

    # A dress-up whose own face or round edge was picked is that feature: editing it is
    # the change the user asked for, not a second feature over its result (which the
    # kernel often refuses, emptying the viewport). Any *other* element of the result is
    # a new fillet, as it should be.
    if (
        element
        and str(getattr(obj, "TypeId", "")) == f"Part::{mode}"
        and _dress_up_target(obj, element) is not None
    ):
        return _edit_dress_up(obj, mode, first, second)

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
        if _invalid(feature):
            doc.abortTransaction()
            return {
                "ok": False,
                "error": f"{mode.lower()} on {element or 'the selection'} failed; the body is unchanged",
            }
        doc.commitTransaction()
    except Exception as exc:
        doc.abortTransaction()
        return {"ok": False, "error": str(exc)}
    return {"ok": True, "feature": feature_name}


FerroCAD.register_display_provider(_display_mesh, default=True)
FerroCAD.register_interaction_provider(_interaction)


#: The smallest radius the scrubber holds; the probe's floor.
_PROBE_FLOOR = 1e-3
#: Halvings once the bracket is found: about a percent of it, finer than a drag can aim.
_PROBE_TRIALS = 6


def _link_sub(obj, name="Base"):
    """``(object, [element names])`` for a ``LinkSub`` property, or ``None``."""
    try:
        value = getattr(obj, name, None)
        if not value:
            return None
        target, elements = value[0], value[1]
    except (TypeError, IndexError, ValueError):
        return None
    if target is None:
        return None
    return target, [str(element) for element in (elements or ())]


def _extrusion_target(obj, element):
    """The existing extrusion that produced ``element``, with its base face.

    A push/pull is a hidden ``Part::Extrusion`` feeding a ``Part::Fuse``/``Part::Cut``.
    The picked cap is the base face translated by the signed length along its normal, so
    the two frames identify it without trusting the lineage (a document saved before the
    kernel recorded it does not carry one). The distance is signed by the extrusion's own
    ``Reversed``.

    Returns ``(extrusion, base object, base element, signed distance)`` or ``None``.
    """
    if str(getattr(obj, "TypeId", "")) not in ("Part::Fuse", "Part::Cut"):
        return None
    tool = getattr(obj, "Tool", None)
    if tool is None or str(getattr(tool, "TypeId", "")) != "Part::Extrusion":
        return None
    linked = _link_sub(tool)
    if linked is None or not linked[1]:
        return None
    base_obj, elements = linked
    distance = float(getattr(tool, "LengthFwd", 0.0) or 0.0)
    if bool(getattr(tool, "Reversed", False)):
        distance = -distance
    if not _is_extrusion_cap(obj, element, base_obj, elements[0], distance):
        return None
    return tool, base_obj, elements[0], distance


def _is_extrusion_cap(obj, element, base_obj, base_element, distance):
    """Whether ``element`` is the face the extrusion's sweep produced.

    The cap is the base face translated by the signed length along its normal. Checking
    that (instead of a name) also covers a document whose stored element map predates the
    provenance the kernel now records.
    """
    if not element.startswith("Face"):
        return False
    base = _face_frame(base_obj, base_element)
    cap = _face_frame(obj, element)
    if base is None or cap is None:
        return False
    normal = base["z"]
    if sum(normal[axis] * cap["z"][axis] for axis in range(3)) < 0.999:
        return False
    spread = max(
        abs(cap["origin"][axis] - (base["origin"][axis] + normal[axis] * distance))
        for axis in range(3)
    )
    try:
        diagonal = float(obj.Shape.BoundBox.DiagonalLength)
    except Exception:  # noqa: BLE001 - a shape without a bound box still has a scale
        diagonal = 1.0
    return spread <= 1e-6 * max(diagonal, 1.0)


def _axis_parallels_an_edge(axis, base_obj, edges):
    """Whether `axis` runs along one of the feature's filleted edges."""
    for edge_name in edges:
        try:
            direction = base_obj.Shape.getElement(edge_name).Curve.Direction
        except Exception:  # noqa: BLE001 - a curved edge has no single direction
            continue
        dot = abs(axis.x * direction.x + axis.y * direction.y + axis.z * direction.z)
        if dot >= 0.999:
            return True
    return False


def _fillet_rounds(obj, base_obj, edges, radius):
    """The faces this fillet created.

    A round is the cylinder of the feature's own radius whose axis runs parallel to a
    filleted edge. Geometry identifies it, so this answers on a document saved before the
    kernel recorded provenance; the surface type keeps a coincidental cylinder out.
    """
    shape = getattr(obj, "Shape", None)
    if shape is None:
        return []
    try:
        parts = [
            name
            for name in list(shape.mesh().get("parts") or ())
            if name.startswith("Face")
        ]
    except Exception:  # noqa: BLE001 - a shape without a mesh has no rounds
        return []
    rounds = []
    for name in parts:
        try:
            surface = shape.getElement(name).Surface
        except Exception:  # noqa: BLE001 - an element the shape cannot resolve
            continue
        if type(surface).__name__ != "Cylinder":
            continue
        try:
            if abs(float(surface.Radius) - radius) > 1e-6 * max(1.0, radius):
                continue
            axis = surface.Axis
        except Exception:  # noqa: BLE001 - a surface without an axis is not a round
            continue
        if _axis_parallels_an_edge(axis, base_obj, edges):
            rounds.append(name)
    return rounds


def _dress_up_target(obj, element):
    """The radius an existing ``Part::Fillet`` holds, when ``element`` belongs to it.

    Two selections re-open the feature: the **round face** it created, and any **edge of
    that round**. The second matters because the compiled edge scrubber arms there, and
    a new fillet on one of the round's own seams is exactly the edit the kernel refuses —
    leaving an invalid feature and an empty viewport. The round's boundary edges are read
    from the face itself (`Face.Edges`), so the test is exact rather than positional.

    A chamfer is deliberately not answered: its distances are not the radius the armed
    scrubber edits.
    """
    if str(getattr(obj, "TypeId", "")) != "Part::Fillet":
        return None
    linked = _link_sub(obj)
    if linked is None or not linked[1]:
        return None
    base_obj, edges = linked
    radius = float(getattr(obj, "Radius", 0.0) or 0.0)
    if radius <= 0.0 or not element:
        return None
    rounds = _fillet_rounds(obj, base_obj, edges, radius)
    if not rounds:
        return None
    if element in rounds:
        return radius
    if not element.startswith("Edge"):
        return None
    try:
        picked = obj.Shape.getElement(element)
    except Exception:  # noqa: BLE001 - an element the shape cannot resolve
        return None
    for name in rounds:
        try:
            boundary = obj.Shape.getElement(name).Edges
        except Exception:  # noqa: BLE001 - a face without edges is not a round
            continue
        if any(picked.isSame(edge) for edge in boundary):
            return radius
    return None


def _face_frame(obj, element):
    """The world frame of a face: origin at its area-weighted centroid, z its normal.

    The mesh the display hook draws is already in world coordinates, so the frame a
    declared handle needs is read straight off it, exactly as :func:`_face_normal` does.
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
    centroid = [0.0, 0.0, 0.0]
    weight = 0.0
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
        cross = (
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        )
        area = 0.5 * math.sqrt(sum(component * component for component in cross))
        weight += area
        for axis in range(3):
            normal[axis] += cross[axis]
            centroid[axis] += area * (a[axis] + b[axis] + c[axis]) / 3.0
    if weight <= 0.0:
        return None
    length = math.sqrt(sum(component * component for component in normal))
    if length <= 1e-12:
        return None
    z = tuple(component / length for component in normal)
    centroid = tuple(component / weight for component in centroid)
    # Any unit vector perpendicular to the normal, then a right-handed y = z x x.
    helper = (1.0, 0.0, 0.0) if abs(z[0]) < 0.9 else (0.0, 1.0, 0.0)
    x = (
        helper[1] * z[2] - helper[2] * z[1],
        helper[2] * z[0] - helper[0] * z[2],
        helper[0] * z[1] - helper[1] * z[0],
    )
    x_length = math.sqrt(sum(component * component for component in x))
    x = tuple(component / x_length for component in x)
    y = (
        z[1] * x[2] - z[2] * x[1],
        z[2] * x[0] - z[0] * x[2],
        z[0] * x[1] - z[1] * x[0],
    )
    return {
        "origin": list(centroid),
        "x": list(x),
        "y": list(y),
        "z": list(z),
    }


#: The declared grip's mesh, in the handle frame's own units (the host scales it to its
#: pixel `size`). A double cone, so it reads as a handle from any camera.
_GRIP_SHAPE = {
    "positions": [
        [0.0, 0.0, -1.0],
        [0.0, 0.0, 1.0],
        [0.16, 0.0, 0.0],
        [0.0, 0.16, 0.0],
        [-0.16, 0.0, 0.0],
        [0.0, -0.16, 0.0],
    ],
    "normals": [
        [0.0, 0.0, -1.0],
        [0.0, 0.0, 1.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, -1.0, 0.0],
    ],
    "triangles": [
        1, 2, 3, 1, 3, 4, 1, 4, 5, 1, 5, 2,
        0, 3, 2, 0, 4, 3, 0, 5, 4, 0, 2, 5,
    ],
    "edges": [],
}


def _manipulator_specs(obj, element):
    """Declared handles that the compiled factories cannot arm.

    The compiled scrubber needs an *edge*, and an applied fillet's round is a *face*:
    the edge it came from no longer exists. A declared grip at the round's centroid,
    seeded with the radius the feature holds, re-opens it — and its commit is the same
    ``dress_up``/``fillet`` the scrubber sends, so the edit path is shared. A round
    *edge* needs no grip: the compiled scrubber arms there and its commit edits the
    feature (see :func:`_dress_up_target`).
    """
    if not element.startswith("Face"):
        return None
    value = _dress_up_target(obj, element)
    if value is None:
        return None
    frame = _face_frame(obj, element)
    if frame is None:
        return None
    linked = _link_sub(obj)
    return [
        {
            "id": "dress_up",
            "action": "fillet",
            "element": linked[1][0] if linked and linked[1] else element,
            "frame": frame,
            "axis": "z",
            "kind": "axis",
            "value": value,
            "min": _PROBE_FLOOR,
            "decimals": 3,
            "label": "R",
            "size": 42.0,
            "shape": _GRIP_SHAPE,
            "readout": "R {value} mm",
        }
    ]


def _fillet_shape_and_edge(obj, element):
    """The shape and edge name a dress-up radius is measured against.

    A selection that belongs to an existing ``Part::Fillet`` — its round, or one of the
    round's edges — is the feature's own result, so the trial fillets run on its *base*
    shape and the edge the feature names. Any other edge, **including another edge of a
    filleted body**, is an edge of the shape on screen: the trial fillets run on that
    shape. A feature selecting several edges gets no range, because one bisection per
    edge over-states the ceiling the combined feature survives.
    """
    if not element:
        return None
    if str(getattr(obj, "TypeId", "")) == "Part::Fillet" and (
        _dress_up_target(obj, element) is not None
    ):
        linked = _link_sub(obj)
        if linked is None or len(linked[1]) != 1:
            return None
        base_obj, edges = linked
        shape = getattr(base_obj, "Shape", None)
        if shape is None:
            return None
        return shape, edges[0]
    if not element.startswith("Edge"):
        return None
    shape = getattr(obj, "Shape", None)
    if shape is None:
        return None
    return shape, element


def _largest_fillet(shape, edge, floor, guess, cap):
    """The largest radius ``shape.makeFillet`` completes on ``edge``, or ``None``.

    ``floor`` is assumed (and verified) to fit. The search grows from ``guess``,
    doubling until something fails, then bisects; past ``cap`` it gives up and reports
    that no ceiling was found rather than inventing one. Every step is a real kernel call
    on the current shape: the document is never touched.
    """

    def fits(radius):
        try:
            filleted = shape.makeFillet(radius, [edge])
        except Exception:  # noqa: BLE001 - a trial that fails is the answer
            return False
        # OCC can return an invalid solid without raising; that is a failure too.
        try:
            return bool(filleted.isValid())
        except Exception:  # noqa: BLE001 - a shape without the check is trusted
            return True

    if not fits(floor):
        return None
    if fits(guess):
        low = guess
        high = guess * 2.0
        while high <= cap and fits(high):
            low, high = high, high * 2.0
        if high > cap:
            # Nothing inside the body's own range failed: there is no ceiling to claim.
            return None
    else:
        low, high = floor, guess
    for _ in range(_PROBE_TRIALS):
        mid = 0.5 * (low + high)
        if fits(mid):
            low = mid
        else:
            high = mid
    return low


def _fillet_bounds(obj, element):
    """The radius range this edge's fillet actually survives, by kernel bisection.

    The shell can prove only the floor from the tessellation — half a body's extent is
    *not* a ceiling, because a single edge of a cube takes a fillet larger than that.
    This asks the kernel instead: ``Shape.makeFillet`` for a series of radii on the
    current shape, so the ceiling is the value that stops working.

    It runs once per arming (the host caches the answer until the shape changes). The
    factory arms fillets only; a chamfer manipulator would need its own probe.
    """
    target = _fillet_shape_and_edge(obj, element)
    if target is None:
        return None
    shape, edge_name = target
    try:
        edge = shape.getElement(edge_name)
        box = shape.BoundBox
    except Exception:
        return None
    lengths = (box.XLength, box.YLength, box.ZLength)
    if min(lengths) <= 0.0:
        return None
    # A working guess and a cap of twice the body's largest extent: past that the feature
    # is meaningless, so "no failure found" is the honest answer.
    guess = max(min(lengths) / 8.0, _PROBE_FLOOR * 2.0)
    cap = 2.0 * max(lengths)
    top = _largest_fillet(shape, edge, _PROBE_FLOOR, guess, cap)
    if top is None:
        return None
    return {
        "min": _PROBE_FLOOR,
        "max": top,
        "note": "kernel bisection (%d halvings)" % _PROBE_TRIALS,
    }


def _manipulator_probe(obj, element, manipulator):
    """What this selection already carries, and the range the kernel completes.

    Push/pull seeds from the extrusion whose cap was picked; a dress-up seeds from the
    fillet whose round (face or edge) was picked. The range is the fillet bisection — the
    only operation here with a ceiling the kernel can prove. When a seed is present its
    note is the user-facing one, because the row prints it: it is how "this edits what
    you already have" is said without a modal.
    """
    if not element:
        # A whole-object selection has no sub-element to follow.
        return None
    if manipulator == "push_pull":
        target = _extrusion_target(obj, element)
        if target is None:
            return None
        return {"values": [target[3]], "note": "editing the existing extrusion"}
    if manipulator != "dress_up":
        return None
    answer = {}
    value = _dress_up_target(obj, element)
    if value is not None:
        answer["values"] = [value]
        answer["note"] = "editing the existing fillet"
    bounds = _fillet_bounds(obj, element)
    if bounds is not None:
        answer["min"] = bounds["min"]
        answer["max"] = bounds["max"]
        if value is None:
            answer["note"] = bounds["note"]
    return answer or None


FerroCAD.register_manipulator_probe(_manipulator_probe)
FerroCAD.register_manipulator_provider(_manipulator_specs)


#: The parametric edition's appearance preset (declared, not yet materialised).
#:
#: Parametric is the neutral edition: no workbench supplies its own surface colour,
#: so Part objects are the opaque steel grey in `DEFAULT_COLOR` (linear
#: ``Y = 0.376``), and the gizmo has no blue object to fight. The preset deliberately pins today's tokens rather than changing
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


def _datum_axis(frame, key):
    """One finite ``[x, y, z]`` axis from a datum-plane payload."""
    value = frame.get(key) if isinstance(frame, dict) else None
    if not isinstance(value, (list, tuple)) or len(value) != 3:
        raise ValueError(f"datum plane {key} must be three numbers")
    try:
        components = tuple(float(component) for component in value)
    except (TypeError, ValueError):
        raise ValueError(f"datum plane {key} must be three numbers") from None
    if not all(math.isfinite(component) for component in components):
        raise ValueError(f"datum plane {key} must be finite")
    return components


def _datum_unit(vector):
    """`vector` scaled to unit length (the caller has already rejected zero)."""
    length = math.sqrt(sum(component * component for component in vector))
    return tuple(component / length for component in vector)


def add_datum_plane(payload: str) -> str:
    """Create a selectable datum plane from the active workplane frame.

    The workplane is transient viewport state, so the shell owns the live frame and
    passes it here when ``App_SaveDatumPlane`` runs: ``origin`` is the plane marker
    in world millimetres, ``u``/``v`` are its in-plane axes and ``u × v`` its
    normal. The frame becomes the object's ``Placement`` (origin + a rotation whose
    local X/Y/Z are ``u``/``v``/``n``); ``Part::DatumPlane.execute`` derives the
    square ``Face1`` from that placement and ``Size`` on recompute.

    The object is added in one undoable transaction. The reply is
    ``{"ok": true, "object": <name>}``, or ``{"ok": false, "error": "..."}`` for a
    malformed or degenerate frame (a zero-length or parallel ``u``/``v``) or a
    kernel failure.
    """
    try:
        frame = json.loads(payload)
        origin = _datum_axis(frame, "origin")
        u = _datum_axis(frame, "u")
        v = _datum_axis(frame, "v")
    except (TypeError, ValueError) as exc:
        return json.dumps({"ok": False, "error": str(exc)})

    length_u = math.sqrt(sum(component * component for component in u))
    length_v = math.sqrt(sum(component * component for component in v))
    if length_u <= 1e-12 or length_v <= 1e-12:
        return json.dumps(
            {"ok": False, "error": "datum plane u and v must be non-zero"}
        )
    normal = (
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    )
    cross = math.sqrt(sum(component * component for component in normal))
    if cross <= 1e-9 * length_u * length_v:
        return json.dumps(
            {"ok": False, "error": "datum plane u and v must not be parallel"}
        )

    # The object's `Placement` is the single source of truth: its rotation maps the
    # local X/Y plane onto this frame. Normalize the basis (the viewport already
    # does, but a script/mod payload need not) and take local Y = n x u, which is
    # right-handed even if the payload's `v` was slightly off-plane.
    u = _datum_unit(u)
    normal = _datum_unit(normal)
    v_unit = (
        normal[1] * u[2] - normal[2] * u[1],
        normal[2] * u[0] - normal[0] * u[2],
        normal[0] * u[1] - normal[1] * u[0],
    )
    try:
        rotation = FreeCAD.Rotation(
            FreeCAD.Vector(*u),
            FreeCAD.Vector(*v_unit),
            FreeCAD.Vector(*normal),
        )
    except Exception as exc:
        return json.dumps({"ok": False, "error": str(exc)})

    doc = FreeCAD.ActiveDocument
    if doc is None:
        doc = FreeCAD.newDocument("Unnamed")
    doc.openTransaction("Datum plane")
    try:
        obj = doc.addObject("Part::DatumPlane", "DatumPlane")
        obj.Label = "DatumPlane"
        obj.Placement = FreeCAD.Placement(FreeCAD.Vector(*origin), rotation)
        # `Size` is an `App::PropertyLength`: the side of the square. The object's
        # `execute` derives the `Face1` shape from `Placement` + `Size` on recompute.
        obj.Size = "100 mm"
        doc.recompute()
        doc.commitTransaction()
    except Exception as exc:
        doc.abortTransaction()
        return json.dumps({"ok": False, "error": str(exc)})
    return json.dumps({"ok": True, "object": obj.Name})


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


# ---------------------------------------------------------------------------
# The boolean and compound commands
# ---------------------------------------------------------------------------
#
# Upstream's `Part_Fuse`/`Part_Cut`/`Part_Common`/`Part_Compound` are thin commands
# over `BOPTools.BOPFeatures` (`Part/Gui/Command.cpp`, `Part/BOPTools/BOPFeatures.py`):
# they police the selection, create one object in one undo step, hide the operands and
# copy the first operand's display attributes. The geometry is the object type's job
# (`Part::MultiFuse` flattens a lone compound; `Part::MultiCommon` honours
# `Behavior`), so nothing here touches shapes.
#
# One deliberate difference: upstream warns before a boolean over non-solids and waits
# for a modal answer. A headless run has to answer for the user, so the command
# proceeds and names the non-solid operands in its status instead of refusing.

_BOOLEAN_COMMANDS = {
    "Part_Fuse": {
        "menu": "Union",
        "tooltip": "Unites the selected shapes",
        "type": "Part::MultiFuse",
        "name": "Fusion",
        "title": "Union",
        "pair": False,
        "verb": "united",
    },
    "Part_Cut": {
        "menu": "Cut",
        "tooltip": "Cuts the second selected shape out of the first",
        "type": "Part::Cut",
        "name": "Cut",
        "title": "Part Cut",
        "pair": True,
        "verb": "cut",
    },
    "Part_Common": {
        "menu": "Intersection",
        "tooltip": "Intersects the selected shapes",
        "type": "Part::MultiCommon",
        "name": "Common",
        "title": "Common",
        "pair": False,
        "verb": "intersected",
    },
}


def _selected_shapes():
    """The selected objects that carry a shape, in selection order, without repeats."""
    objects, seen = [], set()
    for obj in FreeCADGui.Selection.getSelection():
        name = getattr(obj, "Name", None)
        if not name or name in seen:
            continue
        try:
            if obj.Shape is None:
                continue
        except Exception:  # noqa: BLE001 - an object without a Shape is not a candidate
            continue
        seen.add(name)
        objects.append(obj)
    return objects


def _fused_shape_count(objects):
    """How many shapes a fuse would see, following upstream's compound unwrapping.

    `CmdPartFuse::activated` walks into a *lone* compound repeatedly: it counts the
    direct children, then keeps walking while there is exactly one child and that
    child is itself a compound. The object type flattens the same way, so this only
    decides whether the command has enough to work with.
    """
    if len(objects) != 1:
        return len(objects)
    shape = objects[0].Shape
    count = 1
    while count == 1 and shape.ShapeType == "Compound":
        children = shape.childShapes()
        if not children:
            return 0
        count = len(children)
        shape = children[-1]
    return count


def _boolean_refusal(spec, objects):
    """Why the selection cannot run the boolean, or ``None`` when it can."""
    if FreeCAD.ActiveDocument is None:
        return "no active document"
    if spec["pair"]:
        # Upstream's `Part_Cut` is exactly two objects: the first is the base.
        if len(objects) != 2:
            return "select exactly 2 shapes"
        return None
    if _fused_shape_count(objects) < 2:
        where = "select at least 2 shapes"
        if spec["type"] == "Part::MultiFuse":
            where += " (or one compound holding at least two)"
        return where
    return None


def _non_solids(objects):
    """The selected objects whose shape holds no solid."""
    return [
        obj.Name
        for obj in objects
        if not any(child.ShapeType == "Solid" for child in obj.Shape.childShapes())
    ]


def _copy_appearance(target, source):
    """Copy the display attributes upstream's `copy_visual_attributes` copies."""
    for name in ("DisplayMode", "Color"):
        try:
            value = getattr(source, name)
        except Exception:  # noqa: BLE001 - an opened file need not carry them
            continue
        try:
            setattr(target, name, value)
        except Exception:  # noqa: BLE001
            continue


def _create_boolean(spec, objects):
    """Create the boolean over `objects` as one undoable step."""
    doc = FreeCAD.ActiveDocument
    result = None
    doc.openTransaction(spec["title"])
    try:
        result = doc.addObject(spec["type"], spec["name"])
        if spec["pair"]:
            result.Base = objects[0]
            result.Tool = objects[1]
        else:
            result.Shapes = objects
        _add_appearance(result)
        _copy_appearance(result, objects[0])
        doc.recompute()
        # The operands stay in the document (they are the boolean's history) but out
        # of the drawing, the way upstream's `move_input_objects` leaves them.
        for obj in objects:
            try:
                obj.Visibility = False
            except Exception:  # noqa: BLE001
                pass
        doc.commitTransaction()
    except Exception:
        doc.abortTransaction()
        raise
    return result


class _PolylineProxy:
    """A polyline: ``Points``/``Closed`` in, a wire ``Shape`` out.

    A real parametric object rather than a one-shot ``Part::Feature``: writing
    ``Points`` (or ``Closed``) rebuilds the wire on the next recompute, and the
    ``Part::Extrusion`` that links it follows — which is what makes editing the path
    update the pad. The points are world-space, the plane they were drawn on.
    """

    def execute(self, obj):
        points = _polyline_points(list(obj.Points))
        if len(points) < 2:
            obj.Shape = Part.Shape()
            return
        obj.Shape = Part.makePolygon(points, bool(obj.Closed))

    def onDocumentRestored(self, obj):
        self.execute(obj)


def _polyline_points(values):
    """The ``FreeCAD.Vector`` points in a flat ``x, y, z`` list.

    A trailing partial triple is ignored rather than invented: the tool only emits
    complete points, and a hand-edited property should not move a vertex to zero.
    """
    return [
        FreeCAD.Vector(float(values[index]), float(values[index + 1]), float(values[index + 2]))
        for index in range(0, len(values) - 2, 3)
    ]


def _polyline_normal(points):
    """The unit normal of the plane the points span (the extrusion direction).

    The first three non-collinear points define the plane. A straight polyline has
    none, and the world +Z is the only honest guess left; the extrusion then fails
    the same way it would for a profile drawn on a line.
    """
    origin = points[0]
    for index in range(1, len(points) - 1):
        normal = points[index].sub(origin).cross(points[index + 1].sub(origin))
        if normal.Length > 1e-9:
            return normal.normalize()
    return FreeCAD.Vector(0.0, 0.0, 1.0)


def _create_polyline(values):
    """Create the wire and its extrusion from a finished polyline tool commit.

    One named transaction holds both objects, so one undo removes them. The wire
    closes its profile by default — that is what gives the extrusion a face to
    sweep — and is hidden once the solid exists. Editing ``Points``/``Closed`` on
    the wire later rebuilds it, and the linked extrusion follows on recompute.

    An extrusion the kernel refuses does not lose the drawing: the wire is kept,
    the invalid extrusion is removed, and the commit still succeeds.
    """
    if len(values) < 6 or len(values) % 3:
        return {"ok": False, "error": "a polyline needs at least two complete points"}
    points = _polyline_points(values)
    doc = FreeCAD.ActiveDocument
    if doc is None:
        doc = FreeCAD.newDocument("Unnamed")
    doc.openTransaction("Polyline")
    try:
        wire = doc.addObject("Part::FeaturePython", "Polyline", _PolylineProxy(), None)
        wire.addProperty(
            "App::PropertyFloatList", "Points", "Polyline", "World-space vertices"
        )
        wire.addProperty(
            "App::PropertyBool", "Closed", "Polyline", "Close the profile to its start"
        )
        wire.addProperty(
            "App::PropertyLength", "Thickness", "Polyline", "Extrusion height"
        )
        _add_appearance(wire)
        wire.Points = [float(value) for value in values]
        wire.Closed = True
        wire.Thickness = DEFAULT_WIRE_THICKNESS

        extrusion = doc.addObject("Part::Extrusion", f"{wire.Name}_Extrusion")
        extrusion.Base = (wire, [])
        extrusion.DirMode = "Custom"
        extrusion.Dir = _polyline_normal(points)
        extrusion.Solid = True
        extrusion.LengthFwd = float(wire.Thickness)
        _add_appearance(extrusion)
        doc.recompute()
        if _invalid(extrusion):
            # Keep what the user drew; the pad is the part the kernel refused.
            doc.removeObject(extrusion.Name)
            extrusion = None
            doc.recompute()
        else:
            wire.Visibility = False
        if _invalid(wire):
            doc.abortTransaction()
            return {"ok": False, "error": "the polyline could not be built; nothing was added"}
        doc.commitTransaction()
    except Exception as exc:
        doc.abortTransaction()
        return {"ok": False, "error": str(exc)}
    return {"ok": True, "feature": extrusion.Name if extrusion is not None else wire.Name}


class _PolylineCommand:
    """``Part_Polyline`` — draw a polyline on the active workplane and pad it.

    The drawing is the native ``polyline`` tool: clicks place points (snapped to the
    active workplane and its grid), Backspace drops the last, Enter finishes. The
    finished commit creates the wire and its extrusion in one transaction.
    """

    def GetResources(self):
        return {
            "MenuText": "Polyline",
            "ToolTip": "Draw a polyline on the workplane; it is closed and extruded",
            "Group": "Draft",
            "CmdType": "ForEdit",
            "FerroCAD_Surface": "dock",
        }

    def IsActive(self):
        return FreeCAD.ActiveDocument is not None

    def Activated(self, iMsg=0):
        return {"tool": "polyline"}


FreeCADGui.addCommand("Part_Polyline", _PolylineCommand())


class _BooleanCommand:
    """``Part_Fuse`` / ``Part_Cut`` / ``Part_Common`` — one boolean over the selection."""

    def __init__(self, command):
        self.spec = _BOOLEAN_COMMANDS[command]

    def GetResources(self):
        return {
            "MenuText": self.spec["menu"],
            "ToolTip": self.spec["tooltip"],
            "Group": "Part",
            "CmdType": "ForEdit",
            "FerroCAD_Surface": "dock",
        }

    def IsActive(self):
        return (
            FreeCAD.ActiveDocument is not None
            and _boolean_refusal(self.spec, _selected_shapes()) is None
        )

    def Activated(self, iMsg=0):
        objects = _selected_shapes()
        refusal = _boolean_refusal(self.spec, objects)
        if refusal is not None:
            return {"status": refusal}
        result = _create_boolean(self.spec, objects)
        status = f"{self.spec['verb']} {len(objects)} shape(s) into {result.Name}"
        loose = _non_solids(objects)
        if loose:
            status += f" (no solid in {', '.join(loose)})"
        return {"status": status}


for _command in _BOOLEAN_COMMANDS:
    FreeCADGui.addCommand(_command, _BooleanCommand(_command))


class _CompoundCommand:
    """``Part_Compound`` — collect the selected shapes into a ``Part::Compound``.

    Upstream adds no display copy and hides nothing here: every operand stays visible
    and the compound joins them. `Links` keeps the selection order and drops repeated
    picks of one object (`CmdPartCompound::activated`).
    """

    def GetResources(self):
        return {
            "MenuText": "Compound",
            "ToolTip": "Compounds the selected shapes",
            "Group": "Part",
            "CmdType": "ForEdit",
            "FerroCAD_Surface": "dock",
        }

    def IsActive(self):
        return FreeCAD.ActiveDocument is not None and bool(_selected_shapes())

    def Activated(self, iMsg=0):
        objects = _selected_shapes()
        if not objects:
            return {"status": "select at least one shape"}
        doc = FreeCAD.ActiveDocument
        doc.openTransaction("Compound")
        try:
            compound = doc.addObject("Part::Compound", "Compound")
            compound.Links = objects
            _add_appearance(compound)
            doc.recompute()
            doc.commitTransaction()
        except Exception as exc:  # noqa: BLE001 - one transaction, then report
            doc.abortTransaction()
            return {"status": f"compound failed: {exc}"}
        return {"status": f"compounded {len(objects)} shape(s) into {compound.Name}"}


FreeCADGui.addCommand("Part_Compound", _CompoundCommand())


class _SaveDatumPlaneCommand:
    """``App_SaveDatumPlane`` — save the active workplane as a selectable face.

    The plane itself is transient viewport state: the shell reads the live frame and
    calls :func:`add_datum_plane`. A headless run has no viewport, so running the
    command there only reports who owns it.
    """

    def GetResources(self):
        return {
            "MenuText": "Save as datum plane",
            "ToolTip": "Create a selectable plane at the active workplane",
            "Group": "View",
            "CmdType": "ForEdit",
            "FerroCAD_Surface": "workplane",
            "FerroCAD_Parent": "App_Workplane",
            "FerroCAD_Short": "Save WP",
            "FerroCAD_Wheel": True,
            "FerroCAD_Tone": "additive",
        }

    def IsActive(self):
        return FreeCAD.ActiveDocument is not None

    def Activated(self):
        return {"status": "save datum plane: the shell owns this command"}


FreeCADGui.addCommand("App_SaveDatumPlane", _SaveDatumPlaneCommand())


def bootstrap(doc_name: str = "Sample") -> str:
    """Open the canonical sample — the same ``Sample.FCStd`` the browser boots."""
    # Materialise the edition preset under the user's persisted values. The
    # settings facades read that store, so every later command sees the preset.
    FerroCADGui.installAppearancePresets()
    return _base_bootstrap(doc_name)
