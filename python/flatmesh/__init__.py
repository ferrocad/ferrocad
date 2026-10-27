"""Unwrapping a face's triangulation: the part of FreeCAD's ``FlatMesh`` we use.

SimpleCAD wraps an outline onto a curved face and unwraps a face into a flat
outline.  Both go through :class:`FaceUnwrapper`, the one class this module
provides::

    work = face.copy()
    work.tessellate(0.02)                 # any tolerance; see below
    unwrapper = flatmesh.FaceUnwrapper(work)
    unwrapper.findFlatNodes(5, .95)
    unwrapper.nodes                        # (N, 3) 3-D node positions
    unwrapper.ze_nodes                     # (N, 3) flat x, flat y, height
    unwrapper.tris                         # (M, 3) indices into nodes
    unwrapper.getFlatBoundaryNodes()       # list of closed boundary loops

Upstream this is ``src/Mod/MeshPart/App/MeshFlattening*.cpp`` (its Python module
is called ``flatmesh``), which builds a least-squares conformal map of the
face's triangulation and then relaxes it.  This module does **not** implement
that solver.  It produces a defensible unwrap with a much smaller algorithm and
says plainly where the two differ.

What it does
------------

1. **Seam cut.**  A closed face such as a full cylinder wall is one welded
   triangle tube; it cannot be laid flat without tearing.  The face's seam
   edges (``Edge.isSeam(face)``) are sampled, and every mesh node lying on one
   is split into two nodes, one for each side of the cut.  Each side's
   triangles keep their own copy.  This also gives the consumer the duplicated
   3-D nodes it uses to find the seam and repeat the flat sheet across it.
2. **Seed: rigid unfolding.**  Starting at the largest triangle, the mesh is
   laid out one triangle at a time across the dual graph, each new triangle
   placed congruent to its 3-D shape on the far side of the edge it shares with
   a placed one.  Every triangle keeps its exact area and every edge of the
   unfolding tree its exact length.
3. **Relaxation.**  ``findFlatNodes`` then runs up to ``iterations`` spring
   passes over all mesh edges, pulling each node towards the position that
   restores its neighbours' 3-D edge lengths.  On a planar or developable face
   the seed is already exact, the forces are zero, and the passes stop
   immediately.  On a doubly-curved face the seed cannot satisfy every closing
   cycle at once; the passes spread that residual over the mesh instead of
   leaving it on the last triangles placed.

The seed is rigid unfolding rather than upstream's conformal solve, and
deliberately not a projection onto the face's best-fit plane: a half cylinder
wall projects onto its chord plane at ``2/pi`` of its true area, and the five
passes SimpleCAD asks for cannot recover that, while rigid unfolding is exact on
every developable face.  The best-fit plane is used only for the diagnostic
height in the third column of ``ze_nodes``.

The flat coordinates are finally translated so their lower-left corner is at
the origin.  Their orientation is the seed triangle's, not a canonical one:
no consumer needs it, and choosing a "main" direction from a mesh with only a
few rows in one direction is ambiguous.

What the numbers mean
---------------------

``findFlatNodes(iterations, tolerance)``
    ``iterations`` is the maximum number of relaxation passes.  ``tolerance``
    is the relaxation weight in ``(0, 1]``: each pass applies that fraction of
    the correction (upstream calls the same argument ``weight``; SimpleCAD
    passes ``.95``).  It is also the convergence gate: a pass stops the loop
    once it has improved the total edge-length error by less than
    ``1 - tolerance`` (5% at ``.95``).  A value outside the range is clamped to
    ``[.05, 1]``.  Zero iterations leaves the rigid unfolding untouched.

``FaceUnwrapper(face)`` asks for a tessellation fine enough that the flat map
stays inside SimpleCAD's 0.01 mm preview tolerance (``max(.002, min(.5,
sqrt(area) / 5000))``).  The kernel only ever refines a triangulation, never
coarsens it, so a finer mesh the caller already made is kept; a coarser one is
refined.  This is deliberately finer than the tolerance SimpleCAD passes to its
own ``tessellate`` call: the two call sites use the flat mesh as a piecewise
linear approximation of the surface, and a 0.05 mm deflection on a 10 mm radius
cylinder puts the chord midpoint 0.024 mm inside the wall, which the wrap
preview's 0.01 mm tolerance notices.  Passing ``deflection=`` to the
constructor overrides the choice.

``ze_nodes[:, 2]`` is the signed height of each 3-D node above the face's
best-fit plane: the distance the flat sheet would have to be lifted to touch
the surface at that node.  It is zero on a planar face and is a diagnostic, not
something the flat coordinates depend on.  (A face rotationally symmetric about
its own axis has no unique best-fit plane; any plane through the axis fits, so
the sign of the height is arbitrary there.  The x and y columns are unaffected.)

Honest limits
-------------

* **Planar is exact; developable is exact; doubly curved is approximate.**
  Rigid unfolding gives a planar face back exactly and unrolls cylinders,
  cones and other developable faces exactly (their angle defect is zero, so
  every closing cycle meets).  A sphere or a torus cannot be flattened without
  distortion at all: the rigid seed keeps every triangle's area, the relaxation
  then trades that exactness for a lower maximum edge-length error, and
  triangles far from the seed can still overlap and
  ``getFlatBoundaryNodes`` can then describe a self-intersecting outline.
  SimpleCAD treats such an outline as "could not be unwrapped" rather than
  drawing a bad face.
* **This is not a conformal map.**  Upstream minimises a least-squares
  conformal energy before relaxing; the rigid unfolding above is isometric per
  triangle, which is a different objective.  On the faces SimpleCAD exercises
  (planes, cylinder and cone walls) the two agree, because those faces are
  developable.
* **Seam cutting needs a single, clean seam per face.**  Each seam node is
  split by which side of the seam its incident triangles lie on.  A face with
  two crossing seams (a torus) or a non-manifold triangulation is unwrapped as
  far as the cut allows and is otherwise the doubly-curved case above.
* **The triangulation is used as the kernel returns it.**  ``Shape.tessellate``
  already welds coincident vertices; the seam is re-opened here.  A caller that
  passes an unwelded triangulation of its own gets a mesh whose coincident
  nodes are distinct and whose adjacency is therefore not the surface's; use
  the face, not a pre-welded/fractured point list.
* No NumPy.  The edition's embedded interpreter may not have it, and nothing
  here needs it: the algorithm is scalar loops over the node set.

Degenerate input raises :class:`ValueError`: a null or non-face shape, a face
that does not tessellate to at least three nodes and one non-degenerate
triangle, or zero total area.  ``getFlatBoundaryNodes`` before
``findFlatNodes`` also raises.
"""

from __future__ import annotations

import math

__all__ = ["FaceUnwrapper"]

_EPS = 1e-15

#: Fallback tessellation deflection when the face reports no area.
_DEFAULT_DEFLECTION = 0.01


def _sub(a, b):
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])


def _dot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


def _cross(a, b):
    return (
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    )


def _length(a):
    return math.sqrt(a[0] * a[0] + a[1] * a[1] + a[2] * a[2])


def _unit(a):
    length = _length(a)
    if length <= _EPS:
        return None
    return (a[0] / length, a[1] / length, a[2] / length)


def _triangle_area(nodes, triangle):
    a, b, c = nodes[triangle[0]], nodes[triangle[1]], nodes[triangle[2]]
    return 0.5 * _length(_cross(_sub(b, a), _sub(c, a)))


def _triangle_normal(nodes, triangle):
    a, b, c = nodes[triangle[0]], nodes[triangle[1]], nodes[triangle[2]]
    return _cross(_sub(b, a), _sub(c, a))


def _centroid(nodes, triangle):
    a, b, c = nodes[triangle[0]], nodes[triangle[1]], nodes[triangle[2]]
    return (
        (a[0] + b[0] + c[0]) / 3.0,
        (a[1] + b[1] + c[1]) / 3.0,
        (a[2] + b[2] + c[2]) / 3.0,
    )


def _distance(a, b):
    return _length(_sub(a, b))


def _best_fit_normal(points):
    """The eigenvector of the smallest covariance eigenvalue (a unit normal).

    A cyclic Jacobi sweep on the 3x3 covariance; small, symmetric and enough for
    a diagnostic height.  The sign is arbitrary.
    """
    count = len(points)
    cx = sum(p[0] for p in points) / count
    cy = sum(p[1] for p in points) / count
    cz = sum(p[2] for p in points) / count
    m00 = m01 = m02 = m11 = m12 = m22 = 0.0
    for x, y, z in points:
        dx, dy, dz = x - cx, y - cy, z - cz
        m00 += dx * dx
        m01 += dx * dy
        m02 += dx * dz
        m11 += dy * dy
        m12 += dy * dz
        m22 += dz * dz
    scale = float(count)
    a = [
        [m00 / scale, m01 / scale, m02 / scale],
        [m01 / scale, m11 / scale, m12 / scale],
        [m02 / scale, m12 / scale, m22 / scale],
    ]
    v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
    for _ in range(32):
        off = a[0][1] * a[0][1] + a[0][2] * a[0][2] + a[1][2] * a[1][2]
        diagonal = a[0][0] + a[1][1] + a[2][2]
        if off <= (1e-30 * diagonal * diagonal + 1e-300):
            break
        for p, q in ((0, 1), (0, 2), (1, 2)):
            if abs(a[p][q]) <= 1e-300:
                continue
            theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q])
            sign = 1.0 if theta >= 0.0 else -1.0
            t = sign / (abs(theta) + math.sqrt(theta * theta + 1.0))
            c = 1.0 / math.sqrt(t * t + 1.0)
            s = t * c
            for k in range(3):
                akp, akq = a[k][p], a[k][q]
                a[k][p] = c * akp - s * akq
                a[k][q] = s * akp + c * akq
            for k in range(3):
                apk, aqk = a[p][k], a[q][k]
                a[p][k] = c * apk - s * aqk
                a[q][k] = s * apk + c * aqk
            a[p][q] = a[q][p] = 0.0
            for k in range(3):
                vkp, vkq = v[k][p], v[k][q]
                v[k][p] = c * vkp - s * vkq
                v[k][q] = s * vkp + c * vkq
    index = min(range(3), key=lambda i: a[i][i])
    normal = (v[0][index], v[1][index], v[2][index])
    return _unit(normal) or (0.0, 0.0, 1.0)


def _seam_polylines(face, scale):
    """Sampled seam edges as polylines, so nodes on them need no ``Part`` import.

    The sample spacing is a ten-thousandth of the face size (capped at a
    thousand samples per edge), which keeps the chord error far below the
    tolerance used to recognise a seam node.
    """
    try:
        edges = [edge for edge in face.Edges if edge.isSeam(face)]
    except Exception:  # noqa: BLE001 - a surface without seam support has none
        return []
    polylines = []
    spacing = max(scale * 1e-4, 1e-9)
    for edge in edges:
        try:
            length = edge.Length
            count = int(math.ceil(length / spacing))
            count = max(16, min(1024, count))
            sampled = edge.discretize(count)
        except Exception:  # noqa: BLE001 - skip an edge that cannot be sampled
            continue
        polyline = [(point.x, point.y, point.z) for point in sampled]
        if len(polyline) >= 2:
            polylines.append(polyline)
    return polylines


def _closest_on_polyline(point, polyline, maximum):
    """The closest point on the polyline within `maximum`, and its direction.

    Returns ``(distance, direction)`` or ``(None, None)``.  The direction is the
    segment's own direction (first sample to last), so it is consistent along
    the seam.
    """
    best = (None, None)
    for first, second in zip(polyline, polyline[1:]):
        direction = _sub(second, first)
        denominator = _dot(direction, direction)
        if denominator <= _EPS:
            continue
        offset = _sub(point, first)
        parameter = _dot(offset, direction) / denominator
        parameter = 0.0 if parameter < 0.0 else (1.0 if parameter > 1.0 else parameter)
        closest = (
            first[0] + parameter * direction[0],
            first[1] + parameter * direction[1],
            first[2] + parameter * direction[2],
        )
        distance = _distance(point, closest)
        if distance <= maximum and (best[0] is None or distance < best[0]):
            best = (distance, direction)
    return best


def _cut_seams(face, nodes, triangles, scale):
    """Split every seam node into one copy per side of the cut.

    A seam node is a mesh node within a small fraction of the face size from a
    sampled seam edge.  Its incident triangles are grouped by the sign of their
    centroid's displacement along ``normal x seam_direction``: the seam is the
    only place a face's triangles meet from both sides, so the two signs are the
    two sides of the cut.  Nodes with triangles on one side only (a seam end
    that is also a boundary corner) are left alone.
    """
    polylines = _seam_polylines(face, scale)
    if not polylines:
        return nodes, triangles

    tolerance = max(1e-6, scale * 5e-4)
    normals = [[0.0, 0.0, 0.0] for _ in nodes]
    for triangle in triangles:
        normal = _triangle_normal(nodes, triangle)
        for index in triangle:
            current = normals[index]
            current[0] += normal[0]
            current[1] += normal[1]
            current[2] += normal[2]

    seam_nodes = {}
    for index, point in enumerate(nodes):
        best = tolerance
        direction = None
        for polyline in polylines:
            distance, candidate = _closest_on_polyline(point, polyline, best)
            if distance is not None and distance <= best:
                best = distance
                direction = candidate
        if direction is not None:
            seam_nodes[index] = direction
    if not seam_nodes:
        return nodes, triangles

    incident = [[] for _ in nodes]
    for triangle_index, triangle in enumerate(triangles):
        for index in triangle:
            incident[index].append(triangle_index)

    cut = {}
    for index, direction in seam_nodes.items():
        normal = _unit(normals[index])
        tangent = _unit(direction)
        if normal is None or tangent is None:
            continue
        side = _unit(_cross(normal, tangent))
        if side is None:
            continue
        positive = negative = False
        for triangle_index in incident[index]:
            triangle = triangles[triangle_index]
            middle = _centroid(nodes, triangle)
            value = _dot(_sub(middle, nodes[index]), side)
            if value > 0.0:
                positive = True
            elif value < 0.0:
                negative = True
        if positive and negative:
            cut[index] = side
    if not cut:
        return nodes, triangles

    nodes = list(nodes)
    copies = {}
    remapped = []
    for triangle in triangles:
        anchor = None
        for index in triangle:
            if index in cut:
                anchor = index
                break
        if anchor is None:
            remapped.append(triangle)
            continue
        middle = _centroid(nodes, triangle)
        value = _dot(_sub(middle, nodes[anchor]), cut[anchor])
        if value == 0.0:
            remapped.append(triangle)
            continue
        if value < 0.0:
            remapped.append(triangle)
            continue
        new_triangle = []
        for index in triangle:
            if index in cut:
                copy = copies.get(index)
                if copy is None:
                    copy = len(nodes)
                    copies[index] = copy
                    nodes.append(nodes[index])
                new_triangle.append(copy)
            else:
                new_triangle.append(index)
        remapped.append(tuple(new_triangle))

    # Drop any node that the cut left unused (the original of a seam node whose
    # triangles all went to the copy).
    used = {}
    compact = []
    compacted = []
    for triangle in remapped:
        entry = []
        for index in triangle:
            mapped = used.get(index)
            if mapped is None:
                mapped = len(compact)
                used[index] = mapped
                compact.append(nodes[index])
            entry.append(mapped)
        compacted.append(tuple(entry))
    return compact, compacted


def _unfold(nodes, triangles, scale):
    """Rigidly lay the triangles out in 2-D across the dual graph.

    Returns one ``(x, y)`` per node.  Every triangle is placed congruent to its
    3-D shape, on the far side of the edge it shares with the triangle it was
    reached from, so all placed triangles keep the same winding.
    """
    flat = [None] * len(nodes)
    adjacency = {}
    for triangle_index, triangle in enumerate(triangles):
        for k in range(3):
            a, b = triangle[k], triangle[(k + 1) % 3]
            key = (a, b) if a < b else (b, a)
            adjacency.setdefault(key, []).append(triangle_index)

    placed = [False] * len(triangles)
    roots = sorted(
        range(len(triangles)),
        key=lambda i: _triangle_area(nodes, triangles[i]),
        reverse=True,
    )
    offset = 0.0
    for root in roots:
        if placed[root]:
            continue
        triangle = triangles[root]
        a, b, c = triangle
        pa, pb, pc = nodes[a], nodes[b], nodes[c]
        base = _distance(pa, pb)
        if base <= _EPS:
            continue
        from_a = _distance(pa, pc)
        from_b = _distance(pb, pc)
        along = (from_a * from_a - from_b * from_b + base * base) / (2.0 * base)
        height_squared = from_a * from_a - along * along
        height = math.sqrt(height_squared) if height_squared > 0.0 else 0.0
        flat[a] = (offset, 0.0)
        flat[b] = (offset + base, 0.0)
        flat[c] = (offset + along, height)
        placed[root] = True
        queue = [root]
        head = 0
        while head < len(queue):
            triangle_index = queue[head]
            head += 1
            triangle = triangles[triangle_index]
            for k in range(3):
                a, b = triangle[k], triangle[(k + 1) % 3]
                key = (a, b) if a < b else (b, a)
                neighbours = adjacency[key]
                if len(neighbours) != 2:
                    continue
                neighbour = neighbours[0] if neighbours[1] == triangle_index else neighbours[1]
                if placed[neighbour]:
                    continue
                other = triangles[neighbour]
                shared = None
                for j in range(3):
                    u, v = other[j], other[(j + 1) % 3]
                    if (u == a and v == b) or (u == b and v == a):
                        shared = (u, v, other[(j + 2) % 3])
                        break
                if shared is None:
                    continue
                u, v, w = shared
                if flat[u] is None or flat[v] is None:
                    continue
                pu, pv = flat[u], flat[v]
                edge_length = math.hypot(pv[0] - pu[0], pv[1] - pu[1])
                if edge_length <= _EPS:
                    continue
                uw = _distance(nodes[u], nodes[w])
                vw = _distance(nodes[v], nodes[w])
                along = (uw * uw - vw * vw + edge_length * edge_length) / (2.0 * edge_length)
                height_squared = uw * uw - along * along
                height = math.sqrt(height_squared) if height_squared > 0.0 else 0.0
                ex = ((pv[0] - pu[0]) / edge_length, (pv[1] - pu[1]) / edge_length)
                # The neighbour traverses the shared edge as (u, v); putting the
                # third vertex on the left keeps every triangle counter-clockwise.
                left = (-ex[1], ex[0])
                flat[w] = (
                    pu[0] + along * ex[0] + height * left[0],
                    pu[1] + along * ex[1] + height * left[1],
                )
                placed[neighbour] = True
                queue.append(neighbour)

        # Disconnected patches (a non-manifold or split triangulation) are laid
        # out side by side rather than on top of each other.
        lowest = min((p[1] for p in flat if p is not None), default=0.0)
        highest = max((p[1] for p in flat if p is not None), default=0.0)
        widest = max((p[0] for p in flat if p is not None), default=0.0)
        offset = widest + max(highest - lowest, scale) + scale

    # Any node that no triangle uses has no flat position; give it the origin.
    for index, point in enumerate(flat):
        if point is None:
            flat[index] = (0.0, 0.0)
    return flat


def _mesh_edges(nodes, triangles):
    edges = {}
    for triangle in triangles:
        for k in range(3):
            a, b = triangle[k], triangle[(k + 1) % 3]
            key = (a, b) if a < b else (b, a)
            if key not in edges:
                edges[key] = _distance(nodes[a], nodes[b])
    return edges


def _edge_error(flat, edges):
    error = 0.0
    for (a, b), target in edges.items():
        error += abs(math.hypot(flat[a][0] - flat[b][0], flat[a][1] - flat[b][1]) - target)
    return error


def _relax(nodes, triangles, flat, iterations, weight):
    """Spring passes that pull flat edges towards their 3-D lengths.

    Returns the final total edge-length error.  A developable layout is already
    exact, so the first pass measures zero and the loop stops before moving
    anything meaningfully.
    """
    edges = _mesh_edges(nodes, triangles)
    if not edges:
        return 0.0
    neighbours = [[] for _ in flat]
    for (a, b), target in edges.items():
        neighbours[a].append((b, target))
        neighbours[b].append((a, target))

    error = _edge_error(flat, edges)
    for _ in range(max(0, int(iterations))):
        if error <= _EPS:
            break
        before = error
        for index, links in enumerate(neighbours):
            if not links:
                continue
            x, y = flat[index]
            sum_x = sum_y = 0.0
            for other, target in links:
                ox, oy = flat[other]
                dx, dy = x - ox, y - oy
                distance = math.hypot(dx, dy)
                if distance <= _EPS:
                    # A collapsed edge has no direction; leave this term out
                    # rather than inventing one.
                    sum_x += ox
                    sum_y += oy
                    continue
                sum_x += ox + target * dx / distance
                sum_y += oy + target * dy / distance
            count = float(len(links))
            target_x = sum_x / count
            target_y = sum_y / count
            flat[index] = (
                x + weight * (target_x - x),
                y + weight * (target_y - y),
            )
        error = _edge_error(flat, edges)
        if (before - error) <= (1.0 - weight) * before:
            break
    return error


def _boundary_loops(triangles):
    """Closed loops of the triangle edges that appear exactly once.

    Each loop repeats its first index at the end, as upstream's
    ``getFlatBoundaryNodes`` does.  The walk follows the boundary graph one
    edge at a time; a manifold boundary is a set of simple cycles, so it is
    deterministic once the starting edge is chosen.
    """
    counts = {}
    for triangle in triangles:
        for k in range(3):
            a, b = triangle[k], triangle[(k + 1) % 3]
            key = (a, b) if a < b else (b, a)
            counts[key] = counts.get(key, 0) + 1

    adjacency = {}
    for (a, b), count in counts.items():
        if count != 1:
            continue
        adjacency.setdefault(a, []).append(b)
        adjacency.setdefault(b, []).append(a)

    unused = {edge for edge, count in counts.items() if count == 1}
    loops = []
    while unused:
        start, second = min(unused)
        loop = [start]
        previous, current = start, second
        unused.discard((start, second) if start < second else (second, start))
        while True:
            loop.append(current)
            if current == start:
                break
            candidates = [n for n in adjacency.get(current, []) if n != previous]
            following = None
            for candidate in candidates:
                key = (current, candidate) if current < candidate else (candidate, current)
                if key in unused:
                    following = candidate
                    break
            if following is None:
                # A non-manifold boundary (degree not two) ends the walk; close
                # it back to the start only when that edge exists.
                if current != start:
                    loop.append(start)
                break
            key = (current, following) if current < following else (following, current)
            unused.discard(key)
            previous, current = current, following
        if len(loop) >= 4:
            loops.append(loop)
    return loops


class FaceUnwrapper:
    """A face's triangulation, laid flat.

    Construct it from a ``Part.Face`` (a copy is fine; the face's own
    placement is not read, matching upstream), call :meth:`findFlatNodes`, then
    read ``nodes``, ``ze_nodes``, ``tris`` and :meth:`getFlatBoundaryNodes`.
    """

    def __init__(self, face, deflection=None):
        shape_type = getattr(face, "ShapeType", None)
        if shape_type is not None and shape_type != "Face":
            raise ValueError(
                "FaceUnwrapper needs a face, not a %s; select one face and pass it"
                % (shape_type,)
            )
        if not hasattr(face, "tessellate"):
            raise ValueError("FaceUnwrapper needs a Part.Face-like shape that can tessellate")
        if deflection is None:
            area = getattr(face, "Area", 0.0) or 0.0
            if area > 0.0:
                deflection = max(0.002, min(0.5, math.sqrt(area) / 5000.0))
            else:
                deflection = _DEFAULT_DEFLECTION
        try:
            points, facets = face.tessellate(deflection)
        except Exception as error:  # noqa: BLE001 - report the kernel's reason
            raise ValueError("this face has no triangulation to unwrap: %s" % (error,)) from error

        nodes = [(float(point.x), float(point.y), float(point.z)) for point in points]
        if len(nodes) < 3:
            raise ValueError(
                "this face triangulates to %d node%s; a flat mesh needs at least three"
                % (len(nodes), "" if len(nodes) == 1 else "s")
            )
        triangles = []
        for facet in facets:
            if len(facet) != 3:
                continue
            index = (int(facet[0]), int(facet[1]), int(facet[2]))
            if any(not 0 <= value < len(nodes) for value in index):
                raise ValueError("the face's triangulation refers to a node it does not have")
            triangles.append(index)
        if not triangles:
            raise ValueError("this face triangulates to no triangles, so it cannot be unwrapped")

        # Slivers with zero area have no frame to place them in; dropping them
        # also drops the duplicate facets a welding tolerance leaves behind.
        base = 0.0
        for triangle in triangles:
            base = max(base, _distance(nodes[triangle[0]], nodes[triangle[1]]))
        base = max(base, _distance(nodes[triangles[0][1]], nodes[triangles[0][2]]), 1e-12)
        smallest = base * base * 1e-18
        kept = [triangle for triangle in triangles if _triangle_area(nodes, triangle) > smallest]
        if not kept:
            raise ValueError("this face has zero area, so it cannot be unwrapped")
        triangles = kept
        mesh_area = sum(_triangle_area(nodes, triangle) for triangle in triangles)
        if mesh_area <= 0.0:
            raise ValueError("this face has zero area, so it cannot be unwrapped")

        xs = [point[0] for point in nodes]
        ys = [point[1] for point in nodes]
        zs = [point[2] for point in nodes]
        scale = _length(
            (
                max(xs) - min(xs),
                max(ys) - min(ys),
                max(zs) - min(zs),
            )
        )
        scale = max(scale, 1e-9)

        nodes, triangles = _cut_seams(face, nodes, triangles, scale)

        self.nodes = nodes
        self.tris = triangles
        self.ze_nodes = None
        self._scale = scale
        self._normal = _best_fit_normal(nodes)
        centre = (
            sum(point[0] for point in nodes) / len(nodes),
            sum(point[1] for point in nodes) / len(nodes),
            sum(point[2] for point in nodes) / len(nodes),
        )
        self._centre = centre

    def findFlatNodes(self, iterations=5, tolerance=0.95):
        """Lay the mesh out and relax it; fills ``ze_nodes``.

        ``iterations`` spring passes at relaxation weight ``tolerance``; see the
        module docstring for what the numbers mean and when the loop stops.
        """
        weight = float(tolerance)
        if not math.isfinite(weight):
            weight = 1.0
        weight = min(1.0, max(0.05, weight))
        flat = _unfold(self.nodes, self.tris, self._scale)
        _relax(self.nodes, self.tris, flat, iterations, weight)

        minimum_x = min(point[0] for point in flat)
        minimum_y = min(point[1] for point in flat)
        flat = [(point[0] - minimum_x, point[1] - minimum_y) for point in flat]

        centre = self._centre
        normal = self._normal
        self.ze_nodes = [
            (
                point[0],
                point[1],
                _dot(_sub(node, centre), normal),
            )
            for point, node in zip(flat, self.nodes)
        ]
        return None

    def getFlatBoundaryNodes(self):
        """The closed loops of flat boundary points, each repeating its start.

        The points are ``(x, y, 0)`` triples: a 2-D outline living on the flat
        sheet.  A face with holes yields one loop per boundary.
        """
        if self.ze_nodes is None:
            raise ValueError("call findFlatNodes before getFlatBoundaryNodes")
        return [
            [(self.ze_nodes[index][0], self.ze_nodes[index][1], 0.0) for index in loop]
            for loop in _boundary_loops(self.tris)
        ]
