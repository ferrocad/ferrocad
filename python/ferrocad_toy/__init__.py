"""App-shell script for the **toy workbench**.

The kernel-free reference edition. It demonstrates the two module hooks a workbench
needs, with no geometry kernel behind them:

* ``FreeCAD.register_display_provider`` — the slab is a cube generated from its
  ``Height`` property (24 vertices, 6 faces, untagged);
* ``FreeCAD.register_interaction_provider`` — ``toy_height`` / ``set`` writes that
  property in one transaction and recomputes.

The matching Rust manipulator is ``ferrocad_toy::height_grip``, which the edition's
``main`` registers with the shell. Everything that makes the grip work is generic
(``ferrocad_stage::Manipulator`` → ``InteractionCommit`` → this script): swap this
script and the factory for the Part ones and the shell is unchanged.

The shell imports this module by name (``HostConfig.entry_module``), so it must
re-export every entry point the shell calls.
"""

from __future__ import annotations

import FreeCAD

# The shell's data functions (`bootstrap`, `model_tree`, `properties`,
# `set_property`, `evaluate`, `stage_meshes`, `interaction`, ...).
from ferrocad_shell import *  # noqa: F401,F403
from ferrocad_shell import bootstrap as _base_bootstrap

# Half the slab's footprint, in mm.
HALF = 20.0

# Faces of the cube, each counter-clockwise seen from outside. The renderer culls
# back faces, so the winding is the outside.
_FACES = (
    (0, 3, 2, 1),  # bottom (−Z)
    (4, 5, 6, 7),  # top (+Z)
    (0, 1, 5, 4),  # front (−Y)
    (1, 2, 6, 5),  # right (+X)
    (2, 3, 7, 6),  # back (+Y)
    (3, 0, 4, 7),  # left (−X)
)


def _slab(height: float) -> dict:
    """The tessellation of a ``2·HALF`` square slab, ``height`` tall, from z = 0."""
    h = max(float(height), 0.001)
    x0, x1 = -HALF, HALF
    y0, y1 = -HALF, HALF
    corners = [
        (x0, y0, 0.0),
        (x1, y0, 0.0),
        (x1, y1, 0.0),
        (x0, y1, 0.0),
        (x0, y0, h),
        (x1, y0, h),
        (x1, y1, h),
        (x0, y1, h),
    ]
    positions = []
    triangles = []
    edges = []
    for face in _FACES:
        base = len(positions)
        for index in face:
            positions.append(corners[index])
        triangles.extend((base, base + 1, base + 2, base, base + 2, base + 3))
        edges.extend(
            (
                base,
                base + 1,
                base + 1,
                base + 2,
                base + 2,
                base + 3,
                base + 3,
                base,
            )
        )
    return {
        "positions": positions,
        "triangles": triangles,
        # One untagged part: a click selects the whole object (the toy factory's rule).
        "triangle_parts": [0] * (len(triangles) // 3),
        "edges": edges,
        "segment_parts": [0] * (len(edges) // 2),
        "vertices": [],
        "vertex_parts": [],
        "parts": [],
    }


def _display_mesh(obj):
    """The slab mesh for an object with a ``Height``, or ``None``."""
    height = getattr(obj, "Height", None)
    if height is None:
        return None
    try:
        return _slab(float(height))
    except Exception:
        return None


def _interaction(obj, manipulator, action, element, values):
    """Apply the toy height grip: set ``Height`` and recompute.

    The manipulator names its operation and orders the numbers; this hook owns what
    that means for the toy. The edit is one named transaction (a single undo step).
    """
    if manipulator != "toy_height" or action != "set" or not values:
        return None
    doc = obj.Document
    doc.openTransaction(f"Height {obj.Name}")
    try:
        obj.Height = float(values[0])
        doc.recompute()
        doc.commitTransaction()
    except Exception as exc:
        doc.abortTransaction()
        return {"ok": False, "error": str(exc)}
    return {"ok": True}


FreeCAD.register_display_provider(_display_mesh)
FreeCAD.register_interaction_provider(_interaction)


def bootstrap(doc_name: str = "Toy") -> str:
    """The base sample document plus the toy slab."""
    payload = _base_bootstrap(doc_name)
    doc = FreeCAD.ActiveDocument
    slab = doc.addObject("App::FeaturePython", "Slab")
    slab.addProperty("App::PropertyLength", "Height", "Toy", "The slab height")
    slab.Height = 20.0
    doc.recompute()
    return payload
