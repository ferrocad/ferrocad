"""The cage grip's half of the interaction contract.

A cage point drag commits ``cage_point`` / ``move`` with the dragged point's new
world position, and names the point as ``PointN``. This module applies it: one
named transaction, the typed coordinate list, one ``recompute`` — which rebuilds
the preview mesh and the linked skin together, because the skin's `Cage` link
orders it after the cage.

Returning ``None`` for every other verb matters: the facade asks its providers in
order and the first non-``None`` answer owns the commit, so the Sculpt provider
must stay out of the way of Part's dress-up, push/pull and transform.
"""

import FreeCAD

from .display import POINT_PREFIX
from .objects import is_cage

#: The manipulator id the grip reports (see `ferrocad_sculpt::CAGE_POINT_ID`).
MANIPULATOR = "cage_point"

#: The only action a cage point drag produces.
MOVE = "move"


def point_index(element):
    """The cage index a ``PointN`` element names, or ``None``."""
    text = str(element or "")
    if not text.startswith(POINT_PREFIX):
        return None
    try:
        return int(text[len(POINT_PREFIX):])
    except ValueError:
        return None


def interaction(obj, manipulator, action, element, values):
    """Apply a cage point move; ``None`` when the commit is another workbench's."""
    if manipulator != MANIPULATOR or action != MOVE:
        return None
    if not is_cage(obj):
        return {"ok": False, "error": "a cage point move needs a Sculpt cage"}
    index = point_index(element)
    if index is None:
        return {"ok": False, "error": "a cage point move needs a Point<n> element"}
    if len(values) < 3:
        return {"ok": False, "error": "a cage point move needs x, y and z"}

    document = obj.Document
    document.openTransaction("Sculpt cage point")
    try:
        points = list(obj.CagePoints)
        if not 0 <= index < len(points):
            raise ValueError(
                "the cage has %d point(s), so %s%d is not one"
                % (len(points), POINT_PREFIX, index)
            )
        # The grip works in world space (that is what the stage draws); the control
        # points are stored in the object's own frame.
        world = FreeCAD.Vector(float(values[0]), float(values[1]), float(values[2]))
        points[index] = obj.Placement.inverse().multVec(world)
        obj.CagePoints = points
        document.recompute()
        document.commitTransaction()
    except Exception as exc:  # noqa: BLE001 - reported, never raised at the seam
        document.abortTransaction()
        return {"ok": False, "error": str(exc)}
    moved = obj.CagePoints[index]
    return {
        "ok": True,
        "point": index,
        "status": "moved %s%d to %.2f, %.2f, %.2f"
        % (POINT_PREFIX, index, moved.x, moved.y, moved.z),
    }
