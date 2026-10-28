#!/usr/bin/env python3
"""Generate the SVG diagrams used by the general CAD chapters.

The diagrams are committed assets, but their geometry is generated here so the two
views cannot drift into hand-edited styles. Run without arguments to write them, or
with ``--check`` to verify the committed files are current.

Usage::

    python3 tools/generate_concept_diagrams.py
    python3 tools/generate_concept_diagrams.py --check
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
OUT = REPO / "docs" / "general" / "images"

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

    body.append(_box(40, 80, 400, 100, "1. NURBS data", ["control points & weights", "polynomial equations"]))
    body.append(_arrow(240, 180, 240, 212))
    body.append(_text(255, 200, "forms the surfaces of", size=13, fill=ACCENT, weight="600", family=MONO))

    body.append(
        _box(
            40,
            220,
            400,
            120,
            "2. B-Rep data",
            ["geometry: NURBS / planes", "topology: faces, edges,", "vertex connectivity"],
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
        "NURBS and B-Rep are tessellated into a temporary mesh; a permanent mesh arrives "
        "from STL or OBJ; both feed the viewport scene graph and the GPU.",
        "\n".join(body),
    )


def tns_broker() -> str:
    body = []
    body.append('<rect x="80" y="40" width="840" height="120" rx="10" fill="#ffffff" stroke="%s" stroke-width="1.5"/>' % STROKE)
    body.append(_text(98, 68, "1. The geometric drivers", size=17, fill=INK, weight="600"))
    body.append(_chip(110, 82, 240, 52, "Part::TopoShape", "OCCT"))
    body.append(_chip(380, 82, 240, 52, "Mesh", "triangle data"))
    body.append(_chip(650, 82, 240, 52, "Other kernels", "..."))

    body.append(_arrow(500, 160, 500, 222))
    body.append(_text(520, 190, "emits raw mutation events", size=13, fill=ACCENT, weight="600", family=MONO))
    body.append(_text(520, 208, "(input ids, op type, kernel history)", size=12, family=MONO))

    body.append(
        _box(
            180,
            230,
            640,
            100,
            "2. The abstract TNP broker (App core)",
            ["ITopologicalNamingEngine — a proposed interface"],
            fill=SOFT,
        )
    )
    body.append(_arrow(500, 330, 500, 392))
    body.append(_text(520, 365, "routes to the active strategy", size=13, fill=ACCENT, weight="600", family=MONO))

    body.append('<rect x="80" y="400" width="840" height="220" rx="10" fill="#ffffff" stroke="%s" stroke-width="1.5"/>' % STROKE)
    body.append(_text(98, 428, "3. Pluggable TNS implementations", size=17, fill=INK, weight="600"))
    body.append(_box(110, 460, 240, 120, "Genealogy hasher", ["upstream ElementMap", "idea"], title_size=15, body_size=13, leading=18))
    body.append(_box(380, 460, 240, 120, "Spatial heuristic", ["matcher"], title_size=15, body_size=13, leading=18))
    body.append(_box(650, 460, 240, 120, "Hybrid solver", ["solver"], title_size=15, body_size=13, leading=18))
    return _svg(
        1000,
        660,
        "The pluggable topological naming pattern",
        "Geometric drivers emit raw mutation events to an abstract broker, which routes "
        "to one of several naming strategies.",
        "\n".join(body),
    )


DIAGRAMS = {
    "tessellation-pipeline.svg": pipeline,
    "tns-broker.svg": tns_broker,
}


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="verify the committed SVGs are current")
    args = parser.parse_args(argv)

    OUT.mkdir(parents=True, exist_ok=True)
    problems = []
    for name, build in DIAGRAMS.items():
        content = build()
        path = OUT / name
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
