"""The TechDraw surface FerroCAD needs: geometric hatch generation.

FerroCAD does not implement the TechDraw workbench. Draft imports ``TechDraw``
for two functions:

* :func:`makeGeomHatch`, which turns a filled face and a standard AutoCAD
  ``.pat`` pattern into the lines that fill it. This module implements that
  function over the Part kernel.
* :func:`projectToSVG`, the hidden-line-removal projection Draft's SVG exporter
  asks for before falling back to its own edge/arc path. FerroCAD has no HLR
  projection, so it answers ``""`` ("no projection available"), which is the
  empty-snippet case Draft already handles.

Each pattern row is an infinite line family: the row's angle gives the line
direction, its ``(x, y)`` origin and ``(dx, dy)`` offset step place successive
lines, and an optional tail of dash lengths makes the lines dashed. FerroCAD
clips every line to the face with ``Shape.common``, so trimmed and curved
boundaries and faces with holes need no special casing.
"""

import math

import Part

__all__ = ["makeGeomHatch", "projectToSVG"]


def projectToSVG(shape, direction=None, *args, **kwargs):
    """An SVG projection snippet for `shape`, or ``""`` when none is available.

    Upstream this is TechDraw's hidden-line-removal projector. FerroCAD does not
    implement HLR: returning the empty string tells Draft's exporter
    (``draftfunctions/svgshapes.py``) to use its own edge and circular-arc path
    generation, which is the real implementation on this path.
    """
    return ""


def _normalize(vector):
    length = math.sqrt(sum(component * component for component in vector))
    if length == 0.0:
        return (0.0, 0.0, 0.0)
    return tuple(component / length for component in vector)


def _cross(a, b):
    return (
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    )


def _dot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


def _read_patterns(path):
    """The pattern table of an AutoCAD ``.pat`` file.

    Returns ``{name: [row, ...]}`` where a row is the tuple of the numbers on
    one pattern line. Comments (``;``), blank lines and a trailing DOS end-of-
    file marker (``\\x1a``) are ignored, and a row that does not parse as at
    least five numbers is skipped instead of aborting the whole file.
    """
    with open(path, "rb") as pat_file:
        text = pat_file.read().decode("latin-1").replace("\x1a", " ")

    patterns = {}
    current = None
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith(";"):
            continue
        if line.startswith("*"):
            current = line[1:].split(",")[0].strip()
            patterns[current] = []
            continue
        if current is None:
            continue
        numbers = []
        for field in line.split(","):
            field = field.strip()
            if not field:
                continue
            try:
                numbers.append(float(field))
            except ValueError:
                numbers = []
                break
        if len(numbers) >= 5:
            patterns[current].append(tuple(numbers))
    return patterns


def _plane_frame(normal):
    """Two orthonormal axes spanning the plane with `normal`.

    `u` is the world X axis projected onto the plane (the world Y axis when the
    plane contains X), so a pattern angle of 0 is horizontal in the plane's own
    frame, as in TechDraw.
    """
    n = (normal.x, normal.y, normal.z)
    u = (0.0, 0.0, 0.0)
    for world in ((1.0, 0.0, 0.0), (0.0, 1.0, 0.0)):
        along = _dot(world, n)
        u = _normalize(tuple(world[i] - along * n[i] for i in range(3)))
        if u != (0.0, 0.0, 0.0):
            break
    v = _normalize(_cross(n, u))
    return u, v


def _project(point, origin, u, v):
    delta = (point[0] - origin.x, point[1] - origin.y, point[2] - origin.z)
    return (_dot(delta, u), _dot(delta, v))


def _point_on_plane(a, b, origin, u, v):
    return (
        origin.x + a * u[0] + b * v[0],
        origin.y + a * u[1] + b * v[1],
        origin.z + a * u[2] + b * v[2],
    )


def _dash(edge, dashes, scale):
    """Split a clipped straight edge into the pattern's dashes."""
    vertices = edge.Vertexes
    if len(vertices) < 2:
        return []
    start = vertices[0].Point
    end = vertices[-1].Point
    length = edge.Length
    pattern = [abs(value) * scale for value in dashes]
    cycle = sum(pattern)
    if length <= 1e-12 or cycle <= 0.0:
        return [edge]
    unit = _normalize((end.x - start.x, end.y - start.y, end.z - start.z))
    if unit == (0.0, 0.0, 0.0):
        return []

    pieces = []
    position = 0.0
    index = 0
    drawn = True
    while position < length - 1e-12:
        step = pattern[index % len(pattern)]
        index += 1
        if step <= 0.0:
            drawn = not drawn
            continue
        stop = min(position + step, length)
        if drawn:
            pieces.append(
                Part.makeLine(
                    (
                        start.x + unit[0] * position,
                        start.y + unit[1] * position,
                        start.z + unit[2] * position,
                    ),
                    (
                        start.x + unit[0] * stop,
                        start.y + unit[1] * stop,
                        start.z + unit[2] * stop,
                    ),
                )
            )
        position = stop
        drawn = not drawn
    return pieces


def makeGeomHatch(face, scale, pattern, pat_file):
    """The hatch lines of `face` for `pattern` (a row of `pat_file`).

    Parameters
    ----------
    face : Part.Shape
        A planar face to fill.
    scale : float
        Multiplier for the pattern's distances.
    pattern : str
        Case-sensitive pattern name declared in `pat_file`.
    pat_file : str
        Path to an AutoCAD ``.pat`` file.

    Returns
    -------
    Part.Shape
        A compound of the hatch lines. An empty compound when the face is
        outside every line family.
    """
    rows = _read_patterns(pat_file).get(pattern)
    if not rows:
        raise ValueError("pattern '{}' is not declared in '{}'".format(pattern, pat_file))

    scale = float(scale) or 1.0
    plane = face.findPlane()
    if plane is None:
        raise ValueError("TechDraw.makeGeomHatch needs a planar face")

    origin = plane.Location
    u, v = _plane_frame(plane.Axis)

    box = face.BoundBox
    corners = [
        (x, y, z)
        for x in (box.XMin, box.XMax)
        for y in (box.YMin, box.YMax)
        for z in (box.ZMin, box.ZMax)
    ]
    projected = [_project(corner, origin, u, v) for corner in corners]

    edges = []
    for row in rows:
        angle, x, y, dx, dy = row[:5]
        dashes = row[5:]
        theta = math.radians(angle)
        direction = (math.cos(theta), math.sin(theta))
        base = (x * scale, y * scale)
        offset = (dx * scale, dy * scale)

        # Successive lines differ only in this perpendicular coordinate.
        step = direction[0] * offset[1] - direction[1] * offset[0]
        s0 = direction[0] * base[1] - direction[1] * base[0]
        if abs(step) < 1e-12:
            offsets = [0]
        else:
            s_values = [
                direction[0] * point[1] - direction[1] * point[0] for point in projected
            ]
            first = (min(s_values) - s0) / step
            last = (max(s_values) - s0) / step
            offsets = range(math.floor(min(first, last)), math.ceil(max(first, last)) + 1)

        for k in offsets:
            line_origin = (
                base[0] + k * offset[0],
                base[1] + k * offset[1],
            )
            # Run the line past the bounding box on both ends; the kernel trims
            # it back to the face.
            parameters = [
                direction[0] * (point[0] - line_origin[0])
                + direction[1] * (point[1] - line_origin[1])
                for point in projected
            ]
            t_start = min(parameters) - 1.0
            t_end = max(parameters) + 1.0
            start = _point_on_plane(
                line_origin[0] + t_start * direction[0],
                line_origin[1] + t_start * direction[1],
                origin,
                u,
                v,
            )
            end = _point_on_plane(
                line_origin[0] + t_end * direction[0],
                line_origin[1] + t_end * direction[1],
                origin,
                u,
                v,
            )
            clipped = face.common(Part.makeLine(start, end))
            for edge in clipped.Edges:
                if dashes:
                    edges.extend(_dash(edge, dashes, scale))
                else:
                    edges.append(edge)

    return Part.makeCompound(edges)
