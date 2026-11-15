"""The cage's stage row: the subdivision preview plus its control points.

The stage has no notion of "a cage": it draws triangles, edges and vertices, and a
part name per primitive. This module is the bridge. For a Sculpt cage it answers
one row that carries

* the Catmull-Clark preview as the surface (part 0, no element name);
* the **control hull** as edges, also part 0, so the cage reads over the surface;
* every control point as a stage vertex whose part is named ``Point0``,
  ``Point1``, …

That last part is the contract with the cage grip
(`crates/ferrocad_sculpt/src/cage.rs`): the manipulator finds the vertices whose
part name starts with ``Point`` and commits ``cage_point/move`` naming the same
``PointN``, which :mod:`Sculpt.interaction` turns back into ``CagePoints[N]``.

The row is in **world** space: the object's placement is applied here, so the
markers sit on the drawn surface, and the interaction hook inverts it again.
"""

import FreeCAD

from .objects import is_cage
from .subdivision import cage_quads

#: The part-name prefix a control point carries.
POINT_PREFIX = "Point"

#: The cage's surface colour (linear RGBA): a neutral grey so the cyan grip reads.
DEFAULT_COLOR = (0.62, 0.63, 0.66, 1.0)


def control_parts(count):
    """The part names for `count` control points (`["Point0", …]`)."""
    return ["%s%d" % (POINT_PREFIX, index) for index in range(count)]


def cage_display(obj):
    """The stage row for a Sculpt cage, or ``None`` for anything else."""
    if not is_cage(obj):
        return None
    placement = obj.Placement

    # The preview: the proxy's last subdivision, in the object's frame.
    try:
        local_points, facets = obj.Mesh.Topology
    except Exception:
        local_points, facets = [], []
    positions = [
        [point.x, point.y, point.z]
        for point in (placement.multVec(point) for point in local_points)
    ]
    triangles = [index for facet in facets for index in facet]

    # The control hull, as edges over the control points (which come next in the
    # position array, so an index is `base + cage index`). Deduplicated: two faces
    # share each hull edge, and drawing it twice would double the stroke.
    control = [placement.multVec(point) for point in obj.CagePoints]
    base = len(positions)
    positions.extend([point.x, point.y, point.z] for point in control)
    hull = set()
    for face in cage_quads(obj.CageFaces):
        for slot, first in enumerate(face):
            second = face[(slot + 1) % len(face)]
            hull.add((min(first, second), max(first, second)))
    edges = []
    for first, second in sorted(hull):
        edges.extend([base + first, base + second])

    parts = [""] + control_parts(len(control))
    return {
        "positions": positions,
        "triangles": triangles,
        "edges": edges,
        # Every control point is a topological vertex, and its `vertex_parts` entry
        # is 1-based because part 0 is the surface.
        "vertices": list(range(base, base + len(control))),
        "vertex_parts": list(range(1, len(control) + 1)),
        "parts": parts,
        "color": list(DEFAULT_COLOR),
        "display": "shaded_edges",
    }
