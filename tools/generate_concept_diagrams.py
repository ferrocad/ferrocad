#!/usr/bin/env python3
"""Generate the SVG diagrams used by the general CAD chapters and the TNS note.

The diagrams are committed assets, but their geometry is generated here so the two
views cannot drift into hand-edited styles. Run without arguments to write them, or
with ``--check`` to verify the committed files are current.

Usage::

    python3 tools/generate_concept_diagrams.py
    python3 tools/generate_concept_diagrams.py --check
"""

from __future__ import annotations

import argparse
import math
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
GENERAL_IMAGES = REPO / "docs" / "general" / "images"
RESEARCH_IMAGES = REPO / "docs" / "project" / "research" / "images"

BG = "#f8fafc"
INK = "#0f172a"
MUTED = "#475569"
STROKE = "#cbd5e1"
ACCENT = "#2563eb"
SOFT = "#eff6ff"

SANS = "ui-sans-serif, system-ui, -apple-system, 'Segoe UI', sans-serif"
MONO = "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"


def _escape(text: str) -> str:
    return text.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def _text(
    x: float,
    y: float,
    text: str,
    *,
    size: int = 14,
    fill: str = MUTED,
    weight: str = "400",
    anchor: str = "start",
    family: str = SANS,
) -> str:
    return (
        f'<text x="{x}" y="{y}" font-family="{family}" font-size="{size}" '
        f'font-weight="{weight}" fill="{fill}" text-anchor="{anchor}">'
        f"{_escape(text)}</text>"
    )


def _lines(x: float, y: float, lines: list[str], *, size: int = 14, leading: int = 20) -> str:
    return "".join(_text(x, y + index * leading, line, size=size) for index, line in enumerate(lines))


def _box(
    x: float,
    y: float,
    w: float,
    h: float,
    title: str,
    lines: list[str],
    *,
    fill: str = "#ffffff",
    title_size: int = 17,
    body_size: int = 14,
    leading: int = 20,
) -> str:
    return (
        f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="10" '
        f'fill="{fill}" stroke="{STROKE}" stroke-width="1.5"/>'
        + _text(x + 18, y + 28, title, size=title_size, fill=INK, weight="600")
        + _lines(x + 18, y + 52, lines, size=body_size, leading=leading)
    )


def _chip(x: float, y: float, w: float, h: float, title: str, sub: str) -> str:
    return (
        f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="8" '
        f'fill="#ffffff" stroke="{STROKE}" stroke-width="1.2"/>'
        + _text(x + w / 2, y + 21, title, size=14, fill=INK, weight="600", anchor="middle")
        + _text(x + w / 2, y + 40, sub, size=12, fill=MUTED, anchor="middle")
    )


def _arrow(x1: float, y1: float, x2: float, y2: float) -> str:
    return (
        f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" '
        f'stroke="{ACCENT}" stroke-width="2" marker-end="url(#arrow)"/>'
    )


def _path(d: str, *, arrow: bool = True) -> str:
    marker = ' marker-end="url(#arrow)"' if arrow else ""
    return f'<path d="{d}" fill="none" stroke="{ACCENT}" stroke-width="2"{marker}/>'


def _svg(width: int, height: int, title: str, desc: str, body: str) -> str:
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img" aria-labelledby="title desc">
<title id="title">{_escape(title)}</title>
<desc id="desc">{_escape(desc)}</desc>
<defs>
  <marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
    <path d="M 0 0 L 10 5 L 0 10 z" fill="{ACCENT}"/>
  </marker>
</defs>
<rect width="{width}" height="{height}" fill="{BG}"/>
{body}
</svg>
'''


def pipeline() -> str:
    body = []
    body.append(_text(40, 30, "MATHEMATICAL DATA", size=18, fill=INK, weight="700"))
    body.append(_text(40, 52, "smooth · infinite · CPU-bound", size=14))
    body.append(_text(560, 30, "DISCRETE DATA", size=18, fill=INK, weight="700"))
    body.append(_text(560, 52, "faceted · rigid · baked", size=14))

    body.append(
        _box(
            40,
            80,
            400,
            100,
            "1. Mathematical geometry",
            ["analytic surfaces + NURBS", "control points & weights", "polynomial equations"],
        )
    )
    body.append(_arrow(240, 180, 240, 212))
    body.append(_text(255, 200, "forms the surfaces of", size=13, fill=ACCENT, weight="600", family=MONO))

    body.append(
        _box(
            40,
            220,
            400,
            120,
            "2. B-Rep data",
            ["geometry: surfaces", "topology: faces, edges, wires", "tolerances seal the seams"],
        )
    )
    body.append(_arrow(240, 340, 240, 392))
    body.append(_text(255, 370, "tessellated to", size=13, fill=ACCENT, weight="600", family=MONO))

    body.append(
        _box(
            40,
            400,
            400,
            120,
            "3. Temporary visual mesh",
            ["discardable triangles", "regenerates whenever", "B-Rep dimensions change"],
            fill="#f1f5f9",
        )
    )
    body.append(
        _box(
            560,
            400,
            400,
            120,
            "4. Permanent mesh",
            ["fixed triangle faces", "static vertex array", "imported from STL / OBJ"],
            fill="#f1f5f9",
        )
    )

    body.append(_path("M 240 520 L 240 550 L 520 550 L 520 576"))
    body.append(_path("M 760 520 L 760 550 L 520 550", arrow=False))
    body.append(_text(520, 540, "both feed into", size=13, fill=ACCENT, weight="600", anchor="middle", family=MONO))

    body.append(
        _box(
            270,
            580,
            500,
            110,
            "5. Viewport scene graph",
            ["Coin3D / Pivy", "ingests meshes only", "SoIndexedFaceSet (mesh arrays)"],
            fill=SOFT,
        )
    )
    body.append(_arrow(520, 690, 520, 732))
    body.append(
        _box(
            270,
            740,
            500,
            100,
            "6. GPU hardware pipeline",
            ["OpenGL / Vulkan", "vertex shader: 3D → screen", "fragment shader: lighting / color"],
            fill=SOFT,
        )
    )
    return _svg(
        1040,
        880,
        "The CAD data pipeline",
        "Analytic surfaces and NURBS are tessellated into a temporary mesh; a permanent mesh "
        "arrives from STL or OBJ; both feed the viewport scene graph and the GPU.",
        "\n".join(body),
    )


def element_map_layers() -> str:
    body = []

    body.append(
        _box(
            60,
            40,
            600,
            130,
            "App layer  (src/App)",
            [
                "Data::ComplexGeoData: base for shape payloads",
                "Data::ElementMap: stable name -> sub-shape",
                "getElementMapVersion(): versioned contract",
            ],
            fill=SOFT,
        )
    )
    body.append(_arrow(360, 226, 360, 176))
    body.append(_text(375, 206, "names written into the map", size=13, fill=ACCENT, weight="600", family=MONO))
    body.append(_arrow(120, 176, 120, 226))
    body.append(_text(134, 206, "resolve name → sub-shape", size=13, fill=ACCENT, weight="600", family=MONO))

    body.append(
        _box(
            60,
            230,
            600,
            140,
            "Part layer  (src/Mod/Part)",
            [
                "Part::TopoShape wraps OCCT's TopoDS_Shape",
                "Part::TopoShapeMapper / TopoShapeExpansion.cpp",
                "derives names from the kernel's history",
            ],
        )
    )
    body.append(_arrow(360, 426, 360, 376))
    body.append(_text(375, 406, "raw history", size=13, fill=ACCENT, weight="600", family=MONO))
    body.append(_arrow(120, 376, 120, 426))
    body.append(_text(134, 406, "evaluate an operation", size=13, fill=ACCENT, weight="600", family=MONO))

    body.append(
        _box(
            60,
            430,
            600,
            120,
            "OCCT kernel",
            [
                "BRepTools_History: Modified / Generated / Removed",
                "periodic faces carry a seam edge",
                "reports relationships only",
            ],
        )
    )

    body.append(_text(720, 60, "Identity", size=15, fill=INK, weight="600"))
    body.append(_lines(720, 84, ["OCCT never reports a name;", "FreeCAD generates one from", "the relationship graph."], size=13))

    body.append(_text(720, 200, "Persistence", size=15, fill=INK, weight="600"))
    body.append(_lines(720, 224, ["PropertyPartShape saves the", "shape with its map and", "ElementMapVersion. A mismatch", "re-derives instead of trusting", "stale names."], size=13))

    body.append(_text(720, 380, "Not portable", size=15, fill=INK, weight="600"))
    body.append(_lines(720, 404, ["Names depend on kernel", "topology (seam edges) and", "FreeCAD's boolean wrappers.", "A different kernel would", "re-derive, not reuse them."], size=13))

    return _svg(
        1040,
        600,
        "FreeCAD's topological naming split",
        "The App layer stores and versions the element map; the Part layer consumes OCCT "
        "history and derives names; OCCT reports relationships, never names.",
        "\n".join(body),
    )


def tns_broker() -> str:
    body = []
    body.append(_text(40, 28, "RESEARCH SKETCH — no such interface exists in the workspace", size=13, fill="#b45309", weight="600", family=MONO))
    body.append('<rect x="80" y="60" width="840" height="120" rx="10" fill="#ffffff" stroke="%s" stroke-width="1.5"/>' % STROKE)
    body.append(_text(98, 88, "1. The geometric drivers", size=17, fill=INK, weight="600"))
    body.append(_chip(110, 102, 240, 52, "Part::TopoShape", "OCCT"))
    body.append(_chip(380, 102, 240, 52, "Mesh", "triangle data"))
    body.append(_chip(650, 102, 240, 52, "Other kernels", "..."))

    body.append(_arrow(500, 180, 500, 242))
    body.append(_text(520, 210, "emits raw mutation events", size=13, fill=ACCENT, weight="600", family=MONO))
    body.append(_text(520, 228, "(input ids, op type, kernel history)", size=12, family=MONO))

    body.append(
        _box(
            180,
            250,
            640,
            100,
            "2. The abstract TNP broker (App core)",
            ["ITopologicalNamingEngine — a proposed interface"],
            fill=SOFT,
        )
    )
    body.append(_arrow(500, 350, 500, 412))
    body.append(_text(520, 385, "routes to the active strategy", size=13, fill=ACCENT, weight="600", family=MONO))

    body.append('<rect x="80" y="420" width="840" height="220" rx="10" fill="#ffffff" stroke="%s" stroke-width="1.5"/>' % STROKE)
    body.append(_text(98, 448, "3. Pluggable TNS implementations", size=17, fill=INK, weight="600"))
    body.append(_box(110, 480, 240, 120, "Genealogy hasher", ["upstream ElementMap", "idea"], title_size=15, body_size=13, leading=18))
    body.append(_box(380, 480, 240, 120, "Spatial heuristic", ["matcher"], title_size=15, body_size=13, leading=18))
    body.append(_box(650, 480, 240, 120, "Hybrid solver", ["solver"], title_size=15, body_size=13, leading=18))
    return _svg(
        1000,
        680,
        "A proposed broker for topological naming (research sketch)",
        "Geometric drivers would emit raw mutation events to an abstract broker, which would "
        "route to one of several naming strategies. No such interface exists in the workspace.",
        "\n".join(body),
    )


def brep_topology() -> str:
    """The B-Rep ladder: vertex, edge, wire, trimmed face, closed solid."""
    names = ["Vertex", "Edge", "Wire", "Face", "Solid"]
    captions = [
        ["corner point"],
        ["curve between", "two corners"],
        ["closed chain of edges", "an open chain is still a wire"],
        ["surface trimmed", "by a wire"],
        ["closed shell", "with an inside"],
    ]
    panel_w, panel_h, gap = 196, 250, 28
    x0, y0 = 30, 74
    body = []

    for index, (name, caption) in enumerate(zip(names, captions)):
        x = x0 + index * (panel_w + gap)
        cx = x + panel_w / 2
        body.append(
            f'<rect x="{x}" y="{y0}" width="{panel_w}" height="{panel_h}" rx="10" '
            f'fill="#ffffff" stroke="{STROKE}" stroke-width="1.5"/>'
        )
        body.append(
            _text(cx, y0 - 16, f"{index + 1}. {name}", size=15, fill=INK, weight="600", anchor="middle")
        )

        if index == 0:
            body.append(
                f'<circle cx="{cx}" cy="{y0 + 110}" r="17" fill="none" stroke="{STROKE}" '
                'stroke-width="1.5" stroke-dasharray="3 4"/>'
            )
            body.append(f'<circle cx="{cx}" cy="{y0 + 110}" r="6" fill="{ACCENT}"/>')
        elif index == 1:
            body.append(
                f'<line x1="{x + 38}" y1="{y0 + 110}" x2="{x + panel_w - 38}" y2="{y0 + 110}" '
                f'stroke="{INK}" stroke-width="3" stroke-linecap="round"/>'
            )
            for end in (x + 38, x + panel_w - 38):
                body.append(f'<circle cx="{end}" cy="{y0 + 110}" r="5" fill="{ACCENT}"/>')
        elif index in (2, 3):
            left, top, size = cx - 50, y0 + 58, 100
            if index == 3:
                body.append(
                    f'<rect x="{left}" y="{top}" width="{size}" height="{size}" fill="#dbeafe"/>'
                )
                for offset in (0, 30, 60):
                    body.append(
                        f'<line x1="{left}" y1="{top + size - offset}" '
                        f'x2="{left + size - offset}" y2="{top}" stroke="#93c5fd" stroke-width="2"/>'
                    )
            body.append(
                f'<rect x="{left}" y="{top}" width="{size}" height="{size}" fill="none" '
                f'stroke="{INK}" stroke-width="3" stroke-linejoin="round"/>'
            )
            for corner_x, corner_y in (
                (left, top),
                (left + size, top),
                (left + size, top + size),
                (left, top + size),
            ):
                body.append(f'<circle cx="{corner_x}" cy="{corner_y}" r="5" fill="{ACCENT}"/>')
        else:
            # An isometric box: front, top and right faces, with the three hidden
            # edges dashed (a closed shell, not a flat card).
            fx, fy = x + 42, y0 + 100
            width, height, depth = 88, 82, 24
            body.append(
                f'<polygon points="{fx},{fy} {fx + width},{fy} {fx + width},{fy + height} {fx},{fy + height}" '
                'fill="#dbeafe" stroke="none"/>'
            )
            body.append(
                f'<polygon points="{fx},{fy} {fx + depth},{fy - depth} {fx + width + depth},{fy - depth} '
                f'{fx + width},{fy}" fill="#eff6ff" stroke="none"/>'
            )
            body.append(
                f'<polygon points="{fx + width},{fy} {fx + width + depth},{fy - depth} '
                f'{fx + width + depth},{fy + height - depth} {fx + width},{fy + height}" fill="#bfdbfe" stroke="none"/>'
            )
            for edge in (
                f'{fx},{fy} {fx + width},{fy} {fx + width},{fy + height} {fx},{fy + height} {fx},{fy}',
                f'{fx},{fy} {fx + depth},{fy - depth} {fx + width + depth},{fy - depth} '
                f'{fx + width},{fy}',
                f'{fx + width},{fy} {fx + width + depth},{fy - depth} '
                f'{fx + width + depth},{fy + height - depth} {fx + width},{fy + height}',
            ):
                body.append(
                    f'<polyline points="{edge}" fill="none" stroke="{INK}" stroke-width="2.5" '
                    'stroke-linejoin="round"/>'
                )
            # The hidden back corner.
            body.append(
                f'<polyline points="{fx},{fy + height} {fx + depth},{fy + height - depth} '
                f'{fx + width + depth},{fy + height - depth}" fill="none" stroke="#94a3b8" '
                'stroke-width="1.5" stroke-dasharray="4 4"/>'
            )
            body.append(
                f'<line x1="{fx + depth}" y1="{fy - depth}" x2="{fx + depth}" y2="{fy + height - depth}" '
                'stroke="#94a3b8" stroke-width="1.5" stroke-dasharray="4 4"/>'
            )

        for line_index, line in enumerate(caption):
            body.append(
                _text(cx, y0 + panel_h - 40 + line_index * 17, line, size=12, anchor="middle")
            )

        if index < len(names) - 1:
            body.append(_arrow(x + panel_w + 4, y0 + 125, x + panel_w + gap - 4, y0 + 125))

    return _svg(
        1150,
        350,
        "The B-Rep topology ladder",
        "A vertex becomes an edge, edges form a closed wire, the wire trims a face, and "
        "a closed shell of faces bounds a solid. An open chain of edges is still a wire, "
        "but only a closed wire bounds a valid face.",
        "\n".join(body),
    )


def _cylinder(
    cx: float,
    top: float,
    rx: float,
    ry: float,
    height: float,
    sides: int,
    *,
    body: str = "#dbeafe",
    cap: str = "#eff6ff",
    stroke: str = ACCENT,
    facet: str | None = "#93c5fd",
) -> str:
    """A front-view cylinder whose rim is an `sides`-gon.

    The silhouette is exact (vertical sides); the faceting shows on the rim and in
    the vertical seams, which is what a tessellated preview looks like. `facet=None`
    draws the smooth reference shape.
    """
    angles = [2 * math.pi * index / sides for index in range(sides)]
    # The rim nearest the viewer is the half with sin(t) >= 0.
    front = [angle for angle in angles if math.sin(angle) >= -1e-9]

    def rim(angle: float, y: float) -> tuple[float, float]:
        return cx + rx * math.cos(angle), y + ry * math.sin(angle)

    top_rim = [rim(angle, top) for angle in front]
    bottom_rim = [rim(angle, top + height) for angle in front]

    body_points = [(cx - rx, top), *reversed(bottom_rim), (cx + rx, top)]
    body_coords = " ".join(f"{x:.2f},{y:.2f}" for x, y in body_points)
    parts = [f'<polygon points="{body_coords}" fill="{body}" stroke="none"/>']

    if facet is not None:
        for angle in front[1:-1]:
            x, y = rim(angle, top)
            parts.append(
                f'<line x1="{x:.2f}" y1="{y:.2f}" x2="{x:.2f}" y2="{y + height:.2f}" '
                f'stroke="{facet}" stroke-width="1.2" opacity="0.7"/>'
            )

    outline = " ".join(f"{x:.2f},{y:.2f}" for x, y in reversed(bottom_rim))
    parts.append(
        f'<polyline points="{outline}" fill="none" stroke="{stroke}" stroke-width="2.5"/>'
    )
    parts.append(
        f'<line x1="{cx - rx}" y1="{top}" x2="{cx - rx}" y2="{top + height}" '
        f'stroke="{stroke}" stroke-width="2.5"/>'
    )
    parts.append(
        f'<line x1="{cx + rx}" y1="{top}" x2="{cx + rx}" y2="{top + height}" '
        f'stroke="{stroke}" stroke-width="2.5"/>'
    )

    cap_points = " ".join(f"{rim(angle, top)[0]:.2f},{rim(angle, top)[1]:.2f}" for angle in angles)
    parts.append(
        f'<polygon points="{cap_points}" fill="{cap}" stroke="{stroke}" stroke-width="2.5"/>'
    )
    return "".join(parts)


def tessellation_levels() -> str:
    body = []

    body.append(_text(40, 36, "What FreeCAD's math engine knows", size=16, fill=INK, weight="700"))
    body.append(_text(40, 56, "one exact cylinder — 100% round"))
    body.append(
        f'<rect x="40" y="76" width="330" height="310" rx="10" fill="#ffffff" '
        f'stroke="{STROKE}" stroke-width="1.5"/>'
    )
    body.append(_cylinder(205, 150, 110, 22, 170, 96, facet=None))

    body.append(_arrow(384, 230, 496, 230))
    body.append(_text(440, 206, "Tessellation", size=14, fill=ACCENT, weight="600", anchor="middle"))
    body.append(_text(440, 256, "on demand", size=12, anchor="middle"))

    body.append(_text(500, 36, "What your graphics card draws", size=16, fill=INK, weight="700"))
    body.append(_text(500, 56, "triangles only — never the formula"))

    body.append(
        f'<rect x="500" y="76" width="286" height="310" rx="10" fill="#ffffff" '
        f'stroke="{STROKE}" stroke-width="1.5"/>'
    )
    body.append(_text(643, 108, "Coarse preview", size=16, fill=INK, weight="600", anchor="middle"))
    body.append(_text(643, 130, "fast — visibly faceted", size=12, anchor="middle"))
    body.append(_cylinder(643, 170, 90, 18, 150, 8, facet="#64748b"))

    body.append(
        f'<rect x="806" y="76" width="274" height="310" rx="10" fill="#ffffff" '
        f'stroke="{STROKE}" stroke-width="1.5"/>'
    )
    body.append(_text(943, 108, "Fine preview", size=16, fill=INK, weight="600", anchor="middle"))
    body.append(_text(943, 130, "smooth — more triangles", size=12, anchor="middle"))
    body.append(_cylinder(943, 170, 90, 18, 150, 32, facet="#bfdbfe"))

    body.append(
        _text(
            40,
            424,
            "One exact shape in memory. Only the preview mesh changes: the model stays round, "
            "and so does the exported geometry.",
            size=13,
        )
    )

    return _svg(
        1120,
        460,
        "Tessellation levels",
        "An exact cylinder beside a coarse and a fine tessellated preview. Only the "
        "discrete previews are drawn by the GPU; the exact shape is unchanged.",
        "\n".join(body),
    )


def topological_naming() -> str:
    body = []

    def badge(x, y, label, color=INK):
        body.append(
            f'<circle cx="{x}" cy="{y}" r="9" fill="#ffffff" stroke="{color}" stroke-width="1.5"/>'
        )
        body.append(
            _text(x, y + 3.5, label, size=10.5, fill=color, weight="600", anchor="middle")
        )

    def panel(x, y, title, sub, note):
        body.append(
            f'<rect x="{x}" y="{y}" width="330" height="240" rx="10" fill="#ffffff" '
            f'stroke="{STROKE}" stroke-width="1.5"/>'
        )
        body.append(_text(x + 16, y + 28, title, size=15, fill=INK, weight="600"))
        body.append(_text(x + 16, y + 50, sub, size=12))
        body.append(_text(x + 16, y + 224, note, size=11.5))

    def sketch(cx, cy, sides, radius):
        start = math.pi / 4 if sides == 4 else 0.0
        points = [
            (cx + radius * math.cos(start + 2 * math.pi * i / sides),
             cy + radius * math.sin(start + 2 * math.pi * i / sides))
            for i in range(sides)
        ]
        coords = " ".join(f"{x:.1f},{y:.1f}" for x, y in points)
        body.append(
            f'<polygon points="{coords}" fill="#eff6ff" stroke="{INK}" stroke-width="3" '
            'stroke-linejoin="round"/>'
        )
        for x, y in points:
            body.append(f'<circle cx="{x:.1f}" cy="{y:.1f}" r="4" fill="{ACCENT}"/>')

    def box(fx, fy, w, h, d, *, target=None, color=ACCENT, check=False, warn=False):
        ftl, ftr = (fx, fy), (fx + w, fy)
        fbl, fbr = (fx, fy + h), (fx + w, fy + h)
        btl, btr = (fx + d, fy - d), (fx + w + d, fy - d)
        bbl = (fx + d, fy + h - d)

        def poly(points, fill):
            coords = " ".join(f"{x:.1f},{y:.1f}" for x, y in points)
            body.append(f'<polygon points="{coords}" fill="{fill}" stroke="none"/>')

        def line(a, b, stroke=INK, width=2.5, dash=None):
            extra = f' stroke-dasharray="{dash}"' if dash else ""
            body.append(
                f'<line x1="{a[0]:.1f}" y1="{a[1]:.1f}" x2="{b[0]:.1f}" y2="{b[1]:.1f}" '
                f'stroke="{stroke}" stroke-width="{width}" stroke-linecap="round"{extra}/>'
            )

        poly([ftl, ftr, fbr, fbl], "#dbeafe")
        poly([ftl, btl, btr, ftr], "#eff6ff")
        poly([ftr, btr, (btr[0], btr[1] + h), fbr], "#bfdbfe")

        line(ftl, ftr)
        line(ftr, fbr)
        line(fbr, fbl)
        line(fbl, ftl)
        line(ftl, btl)
        line(btl, btr)
        line(btr, (btr[0], btr[1] + h))
        line((btr[0], btr[1] + h), fbr)

        line(fbl, bbl, stroke="#94a3b8", width=1.5, dash="4 4")
        line(bbl, (btr[0], btr[1] + h), stroke="#94a3b8", width=1.5, dash="4 4")
        line(btl, bbl, stroke="#94a3b8", width=1.5, dash="4 4")

        # The four top edges are 1..4 in this illustration; #4 is top-right.
        edges = {1: (btl, btr), 2: (btl, ftl), 3: (ftl, ftr), 4: (ftr, btr)}
        labels = {1: (0, -12), 2: (-14, 0), 3: (0, -12), 4: (14, 0)}
        for number, (a, b) in edges.items():
            mid = ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2)
            if target == number:
                line(a, b, stroke=color, width=6)
            dx, dy = labels[number]
            badge(mid[0] + dx, mid[1] + dy, str(number), color=color if target == number else INK)

        if target is not None:
            a, b = edges[target]
            mid = ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2)
            if check:
                x, y = mid[0] + 22, mid[1] - 18
                body.append(
                    f'<polyline points="{x},{y} {x + 5},{y + 6} {x + 15},{y - 9}" fill="none" '
                    f'stroke="#16a34a" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>'
                )
            if warn:
                x, y = mid[0] + 26, mid[1] - 16
                body.append(
                    f'<polygon points="{x},{y - 11} {x + 12},{y + 9} {x - 12},{y + 9}" '
                    'fill="#fee2e2" stroke="#dc2626" stroke-width="1.5"/>'
                )
                body.append(_text(x, y + 5, "!", size=13, fill="#dc2626", weight="700", anchor="middle"))

    def prism(cx, top, rx, ry, height, *, target_bottom=None, intended_top=None, warn=False):
        sides = 6

        def point(angle, y):
            return (cx + rx * math.cos(angle), y + ry * math.sin(angle))

        body.append(_cylinder(cx, top, rx, ry, height, sides, facet="#64748b"))

        # The bottom-front rim edge that a positional #4 now selects.
        a, b = point(2 * math.pi / 3, top + height), point(math.pi, top + height)
        if target_bottom:
            body.append(
                f'<line x1="{a[0]:.1f}" y1="{a[1]:.1f}" x2="{b[0]:.1f}" y2="{b[1]:.1f}" '
                'stroke="#dc2626" stroke-width="6" stroke-linecap="round"/>'
            )
            mid = ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2)
            badge(mid[0], mid[1] - 18, "4", color="#dc2626")
            if warn:
                x, y = mid[0] + 24, mid[1] - 20
                body.append(
                    f'<polygon points="{x},{y - 11} {x + 12},{y + 9} {x - 12},{y + 9}" '
                    'fill="#fee2e2" stroke="#dc2626" stroke-width="1.5"/>'
                )
                body.append(_text(x, y + 5, "!", size=13, fill="#dc2626", weight="700", anchor="middle"))

        # The physical edge that #4 used to name, now a different number.
        c, d = point(5 * math.pi / 3, top), point(0, top)
        if intended_top:
            body.append(
                f'<line x1="{c[0]:.1f}" y1="{c[1]:.1f}" x2="{d[0]:.1f}" y2="{d[1]:.1f}" '
                'stroke="#94a3b8" stroke-width="2.5" stroke-dasharray="5 4"/>'
            )
            badge((c[0] + d[0]) / 2 + 12, (c[1] + d[1]) / 2 - 6, "9", color="#64748b")

    body.append(_text(40, 30, "A. The original recipe", size=17, fill=INK, weight="700"))
    body.append(_text(40, 340, "B. Edit step 1, rebuild the rest", size=17, fill=INK, weight="700"))

    y_a = 50
    panel(40, y_a, "1. Square sketch", "4 edges", "one outline")
    sketch(205, y_a + 135, 4, 60)
    panel(410, y_a, "2. Extrude — 12 edges", "#4 is the top-right edge", "edges numbered in traversal order")
    box(560, y_a + 110, 100, 84, 26, target=4, color=ACCENT)
    panel(780, y_a, "3. Fillet Edge #4", "lands on the intended edge", "the recipe holds")
    box(930, y_a + 110, 100, 84, 26, target=4, color="#16a34a", check=True)

    y_b = 360
    panel(40, y_b, "1. Hexagon sketch", "6 edges", "a small edit")
    sketch(205, y_b + 135, 6, 60)
    panel(410, y_b, "2. Extrude — 18 edges", "everything is renumbered", "the same edge is now #9")
    prism(575, y_b + 85, 70, 20, 95, target_bottom=True, intended_top=True)
    panel(780, y_b, "3. Fillet Edge #4", "lands on the wrong edge", "a silent wrong edge")
    prism(945, y_b + 85, 70, 20, 95, target_bottom=True, warn=True)

    for row_y in (y_a, y_b):
        body.append(_arrow(374, row_y + 120, 406, row_y + 120))
        body.append(_arrow(744, row_y + 120, 776, row_y + 120))

    body.append(
        _text(
            40,
            636,
            "The recipe is unchanged; the reference is not. Numbers are illustrative — FreeCAD derives them from traversal order.",
            size=13,
        )
    )

    return _svg(
        1120,
        660,
        "The topological naming problem",
        "Two rows: a square extruded and filleted on edge 4; then a hexagon extruded, "
        "where positional numbering moves edge 4 to the bottom and the fillet lands "
        "on the wrong edge.",
        "\n".join(body),
    )


DIAGRAMS = [
    (GENERAL_IMAGES, "tessellation-pipeline.svg", pipeline),
    (GENERAL_IMAGES, "tessellation-levels.svg", tessellation_levels),
    (GENERAL_IMAGES, "element-map-layers.svg", element_map_layers),
    (GENERAL_IMAGES, "brep-topology.svg", brep_topology),
    (GENERAL_IMAGES, "topological-naming.svg", topological_naming),
    (RESEARCH_IMAGES, "tns-broker.svg", tns_broker),
]


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="verify the committed SVGs are current")
    args = parser.parse_args(argv)

    problems = []
    for directory, name, build in DIAGRAMS:
        directory.mkdir(parents=True, exist_ok=True)
        content = build()
        path = directory / name
        if args.check:
            if not path.is_file():
                problems.append(f"missing {path.relative_to(REPO)}")
            elif path.read_text() != content:
                problems.append(f"stale {path.relative_to(REPO)}")
        else:
            path.write_text(content)
            print(f"wrote {path.relative_to(REPO)}")

    if problems:
        for problem in problems:
            print(f"error: {problem}", file=sys.stderr)
        return 1
    if args.check:
        print("concept diagrams are current")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
