"""Catmull-Clark subdivision of a polygon cage.

The cage is what the Sculpt workbench edits: a list of points and a list of
polygons over them (quads in the SubD layout, but any polygon is accepted). One
`catmull_clark` call is one refinement: every face becomes one quad per corner,
so a quad cage stays a quad cage.

The rules are the standard ones, including the boundary rules, because a cage is
routinely left open while it is being built:

* a face point is the centroid of its face;
* an interior edge point averages its two endpoints and the two adjacent face
  points, a boundary edge point is its midpoint;
* an interior vertex with valence ``n`` moves to ``(F + 2R + (n - 3)P) / n``
  (``F`` the average of the adjacent face points, ``R`` the average of the
  adjacent edge midpoints);
* a boundary vertex with one incident face, or with a boundary fan that is not a
  single chain, is a **corner** and stays put — that is what keeps an open
  cage's outline where the user put it;
* any other boundary vertex (two boundary edges, two or more faces) follows the
  ``(P_prev + 6P + P_next) / 8`` curve rule.

An edge used by more than two faces is treated as a boundary, so a non-manifold
cage refines without tearing instead of failing.

The output point order is stable and is part of the contract — original points
first (moved), then one point per face in face order, then one per edge in
first-seen order — because the patch builder and the preview read it by index.
"""

import FreeCAD


def cage_quads(flat):
    """The cage's faces from a flat ``CageFaces`` integer list.

    The SubD workbench stores four indices per face; this accepts any flat
    multiple-of-four list and rejects anything else.
    """
    flat = list(flat)
    if len(flat) % 4:
        raise ValueError("CageFaces holds four indices per face")
    return [tuple(flat[i:i + 4]) for i in range(0, len(flat), 4)]


def catmull_clark(points, faces):
    """One Catmull-Clark refinement of ``(points, faces)``.

    Returns ``(points, quads)``. The input is not modified.
    """
    points = [FreeCAD.Vector(point) for point in points]
    faces = [tuple(face) for face in faces]
    if not faces:
        return points, []
    for face in faces:
        if len(face) < 3:
            raise ValueError("a cage face needs at least three corners")
        for index in face:
            if index < 0 or index >= len(points):
                raise ValueError("a cage face references a missing point")

    face_points = []
    for face in faces:
        total = FreeCAD.Vector()
        for index in face:
            total += points[index]
        face_points.append(total * (1.0 / len(face)))

    # Undirected edge -> the faces that use it, and its output index.
    edges = {}
    for fi, face in enumerate(faces):
        for k, a in enumerate(face):
            b = face[(k + 1) % len(face)]
            key = (a, b) if a < b else (b, a)
            edges.setdefault(key, []).append(fi)
    edge_index = {}
    for key in edges:
        edge_index[key] = len(points) + len(faces) + len(edge_index)

    # Points, in the documented order: vertices, face points, edge points.
    new_points = list(points) + list(face_points)
    for key, adjacent in edges.items():
        a, b = key
        midpoint = (points[a] + points[b]) * 0.5
        if len(adjacent) == 2:
            midpoint = (
                midpoint + (face_points[adjacent[0]] + face_points[adjacent[1]]) * 0.5
            ) * 0.5
        new_points.append(midpoint)

    incident_faces = [[] for _ in points]
    incident_edges = [[] for _ in points]
    for fi, face in enumerate(faces):
        for index in face:
            incident_faces[index].append(fi)
    for key in edges:
        incident_edges[key[0]].append(key)
        incident_edges[key[1]].append(key)

    for vi, point in enumerate(points):
        on_boundary = [key for key in incident_edges[vi] if len(edges[key]) != 2]
        if on_boundary:
            # A boundary vertex with a single incident face (the corner of an open
            # patch) or with a fan that is not one chain is a corner: keeping it
            # fixed is what holds an open cage's outline. The rest smooth along
            # the boundary with the (1, 6, 1)/8 curve rule.
            if len(on_boundary) == 2 and len(incident_faces[vi]) > 1:
                neighbours = [key[0] if key[1] == vi else key[1] for key in on_boundary]
                new_points[vi] = (
                    points[neighbours[0]] + points[neighbours[1]] + point * 6.0
                ) * 0.125
            continue
        else:
            valence = len(incident_faces[vi])
            if valence == 0:
                continue
            face_average = FreeCAD.Vector()
            for fi in incident_faces[vi]:
                face_average += face_points[fi]
            face_average *= 1.0 / valence
            edge_average = FreeCAD.Vector()
            for key in incident_edges[vi]:
                edge_average += (points[key[0]] + points[key[1]]) * 0.5
            edge_average *= 1.0 / len(incident_edges[vi])
            new_points[vi] = (
                face_average + edge_average * 2.0 + point * (valence - 3.0)
            ) * (1.0 / valence)

    quads = []
    for fi, face in enumerate(faces):
        count = len(face)
        for k, corner in enumerate(face):
            previous = face[(k - 1) % count]
            following = face[(k + 1) % count]
            before = edge_index[
                (previous, corner) if previous < corner else (corner, previous)
            ]
            after = edge_index[
                (corner, following) if corner < following else (following, corner)
            ]
            quads.append((corner, after, len(points) + fi, before))
    return new_points, quads


def triangles(quads):
    """Two triangles per quad, for a ``Mesh.Mesh`` preview."""
    result = []
    for a, b, c, d in quads:
        result.append((a, b, c))
        result.append((a, c, d))
    return result
