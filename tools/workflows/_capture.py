"""The capture protocol shared by the user-documentation workflows.

A workflow is an ordinary Python script that drives the real ``FreeCAD`` facade.
It records each step through a :class:`Capture`, and ``tools/userdocs.py`` turns
that record into a markdown page.

The point is that nothing in a page is retyped: a code block is the exact source
that was executed, and a table or JSON block is the value read back afterwards.
A page therefore cannot describe behaviour the code no longer has. If a step
breaks, generation fails; if a value changes, ``--check`` reports the page as
stale.

Workflow contract::

    SLUG = "documents-and-properties"     # page name
    QUADRANT = "how-to"                   # tutorials | how-to | explanation
    TITLE = "..."                         # page title
    SUMMARY = "..."                       # one paragraph
    REQUIRES = ["..."]                    # what a reader needs to reproduce it

    def run(cap):
        with cap.step("Create a document"):
            cap.prose("...")
            cap.exec("doc = FreeCAD.newDocument('Demo')")
            cap.result("Active document", FreeCAD.ActiveDocument.Name)

    if __name__ == "__main__":
        main(run, slug=SLUG, quadrant=QUADRANT, title=TITLE, summary=SUMMARY, requires=REQUIRES)

The quadrant is the Diátaxis section the page belongs to; the runner writes the
page under ``docs/user/<quadrant>/`` and groups the index the same way.

``cap.image()`` records a render spec (a mesh or box, plus a fixed camera). The
page links the committed PNG under ``docs/user/images/``; ``tools/userdocs.py
--capture`` renders the spec with the real offscreen renderer. The page itself
and ``--check`` never need a GPU.

Run a single workflow directly to see its capture as JSON; the runner is
``tools/userdocs.py``.
"""

from __future__ import annotations

import contextlib
import inspect
import json
import textwrap

import FreeCAD
import FreeCADGui

#: Diátaxis sections a workflow may declare, and their aliases. The runner routes
#: both the page path and the index section from this, so the set is closed.
QUADRANTS = {
    "tutorials": "tutorials",
    "tutorial": "tutorials",
    "how-to": "how-to",
    "howto": "how-to",
    "explanation": "explanation",
}


class Capture:
    """Records the ordered operations that make up one workflow page."""

    def __init__(self, *, slug, quadrant="how-to", title, summary, requires=(), caveats=()):
        self.slug = slug
        if quadrant not in QUADRANTS:
            raise ValueError(
                f"unknown quadrant {quadrant!r}; expected one of "
                f"{', '.join(sorted(set(QUADRANTS.values())))}"
            )
        self.quadrant = QUADRANTS[quadrant]
        self.title = title
        self.summary = summary
        self.requires = list(requires)
        self.caveats = list(caveats)
        self.ops: list[dict] = []
        self._executed = 0

    # -- structure ---------------------------------------------------------
    @contextlib.contextmanager
    def step(self, caption: str):
        """Start a titled step. Everything recorded inside belongs to it."""
        self.ops.append({"kind": "step", "caption": caption})
        yield self

    # -- content -----------------------------------------------------------
    def prose(self, text: str) -> None:
        """A markdown paragraph, emitted verbatim."""
        self.ops.append({"kind": "prose", "text": textwrap.dedent(text).strip()})

    def note(self, text: str) -> None:
        """A short aside, rendered as a block quote."""
        self.ops.append({"kind": "note", "text": textwrap.dedent(text).strip()})

    def exec(self, source: str) -> None:
        """Record ``source`` as a code block and execute it.

        The source runs in the workflow module's globals, so a name bound by one
        block is readable by the next one and by the Python that follows it. The
        recorded block is therefore exactly what ran, in the order it ran.
        """
        src = textwrap.dedent(source).strip("\n")
        self._executed += 1
        self.ops.append({"kind": "code", "source": src})
        code = compile(src, f"<workflow {self.slug} #{self._executed}>", "exec")
        caller = inspect.currentframe().f_back
        exec(code, caller.f_globals)  # noqa: S102 - the workflow author supplies this source

    def result(self, label: str, value) -> None:
        """Append a ``(label, value)`` row to the current step's result table."""
        if self.ops and self.ops[-1]["kind"] == "table":
            self.ops[-1]["rows"].append([label, _cell(value)])
        else:
            self.ops.append(
                {
                    "kind": "table",
                    "headers": ["Read back", "Value"],
                    "rows": [[label, _cell(value)]],
                }
            )

    def table(self, headers, rows) -> None:
        """Append a table with explicit headers."""
        self.ops.append(
            {
                "kind": "table",
                "headers": [str(h) for h in headers],
                "rows": [[_cell(c) for c in row] for row in rows],
            }
        )

    def json(self, label: str, value) -> None:
        """Append a labelled JSON snapshot."""
        self.ops.append(
            {
                "kind": "json",
                "label": label,
                "text": json.dumps(value, indent=2, sort_keys=True, default=_default),
            }
        )

    def image(
        self,
        name: str,
        alt: str,
        caption: str | None = None,
        *,
        shape=None,
        mesh=None,
        camera=None,
        size=None,
        color=None,
        edge_color=None,
        draw_mode=None,
    ) -> None:
        """Record a viewport snapshot and the spec that renders it.

        ``name`` is a file name under ``docs/user/images/``; the page links it when
        the file exists and says so when it does not, so the documentation gate
        never needs a GPU. ``tools/userdocs.py --capture`` renders the spec into
        that file with the real offscreen renderer.

        Exactly one of ``shape`` or ``mesh`` supplies the geometry:

        * ``shape`` — either a mapping in renderer form (``{"box": {...}}`` or
          ``{"mesh": {...}}``), or an object with ``tessellate(tolerance)`` (a
          kernel shape). The kernel shape is tessellated here, in the workflow's
          own process, because the renderer is kernel-free.
        * ``mesh`` — a mapping with ``positions``/``triangles``/``edges`` (and
          optionally ``normals``), such as one item from
          ``ferrocad_shell.stage_meshes()``. Its ``color`` and ``display`` keys are
          carried into the spec when they are present.

        ``camera`` is ``{"eye": [...], "target": [...], "up": [...],
        "fov_y": ...}``; the eye and target are required, so every page pins the
        view it documents. ``size`` is ``[width, height]`` (the renderer's default
        when omitted).
        """
        spec = None
        if shape is not None or mesh is not None:
            if shape is not None and mesh is not None:
                raise ValueError("image(): pass either shape= or mesh=, not both")
            if camera is None:
                raise ValueError("image(): a snapshot needs an explicit camera=")
            entry = _geometry_entry(shape if shape is not None else mesh, is_mesh=mesh is not None)
            if color is None and isinstance(mesh, dict):
                color = mesh.get("color")
            if draw_mode is None and isinstance(mesh, dict):
                draw_mode = mesh.get("display")
            if color is not None:
                entry["color"] = [float(component) for component in color]
            if edge_color is not None:
                entry["edge_color"] = [float(component) for component in edge_color]
            if draw_mode is not None:
                entry["draw_mode"] = str(draw_mode)
            spec = {"camera": _camera_spec(camera), "shapes": [entry]}
            if size is not None:
                if len(size) != 2:
                    raise ValueError("image(): size= must be [width, height]")
                spec["size"] = [int(size[0]), int(size[1])]
        self.ops.append(
            {
                "kind": "image",
                "name": name,
                "alt": alt,
                "caption": caption,
                "spec": spec,
            }
        )

    # -- output ------------------------------------------------------------
    def to_dict(self) -> dict:
        return {
            "slug": self.slug,
            "quadrant": self.quadrant,
            "title": self.title,
            "summary": self.summary,
            "requires": self.requires,
            "caveats": self.caveats,
            "ops": self.ops,
        }


def _geometry_entry(value, *, is_mesh: bool) -> dict:
    """The renderer's geometry entry for a ``shape=``/``mesh=`` argument."""
    if is_mesh or (isinstance(value, dict) and "positions" in value):
        if not isinstance(value, dict):
            raise ValueError("image(): mesh= must be a mapping with positions/triangles/edges")
        if not value.get("positions"):
            raise ValueError("image(): mesh= has no positions")
        mesh = {
            "positions": [[float(c) for c in point] for point in value["positions"]],
            "normals": [[float(c) for c in point] for point in value.get("normals", [])],
            "triangles": [int(index) for index in value.get("triangles", [])],
            "edges": [int(index) for index in value.get("edges", [])],
        }
        return {"geometry": {"mesh": mesh}}

    if isinstance(value, dict):
        if "box" in value or "mesh" in value:
            return {"geometry": value}
        raise ValueError(
            "image(): shape= must be {'box': ...}, {'mesh': ...}, or a tessellatable shape"
        )

    if hasattr(value, "tessellate"):
        return {"geometry": {"mesh": _tessellate(value)}}

    raise ValueError(
        "image(): shape= must be {'box': ...}, {'mesh': ...}, or a tessellatable shape"
    )


def _tessellate(shape, tolerance: float = 0.1) -> dict:
    """Tessellate a kernel shape in the workflow process.

    The renderer is kernel-free, so the producer (this process, which has the
    kernel) supplies the triangles. Surface vertices come from ``tessellate``;
    each edge's polyline points are appended to the same position list and indexed,
    which is the layout ``ferrocad_stage::ShapeItem`` expects.
    """
    vertices, facets = shape.tessellate(tolerance)
    positions = [[float(v.x), float(v.y), float(v.z)] for v in vertices]
    triangles = [int(index) for facet in facets for index in facet]
    edges = []
    for edge in getattr(shape, "Edges", []):
        indices = []
        previous = None
        for point in edge.discretize(16):
            current = [float(point.x), float(point.y), float(point.z)]
            if current == previous:
                continue
            indices.append(len(positions))
            positions.append(current)
            previous = current
        for start, end in zip(indices, indices[1:]):
            edges.extend([start, end])
    return {
        "positions": positions,
        "normals": [],
        "triangles": triangles,
        "edges": edges,
    }


def _camera_spec(camera) -> dict:
    """Validate and normalise an ``image(camera=...)`` mapping."""
    if not isinstance(camera, dict):
        raise ValueError("image(): camera= must be a mapping")
    spec = {}
    for key in ("eye", "target"):
        if key not in camera:
            raise ValueError(f"image(): camera= needs {key!r}")
        point = camera[key]
        if len(point) != 3:
            raise ValueError(f"image(): camera {key!r} needs three numbers")
        spec[key] = [float(component) for component in point]
    if "up" in camera:
        if len(camera["up"]) != 3:
            raise ValueError("image(): camera 'up' needs three numbers")
        spec["up"] = [float(component) for component in camera["up"]]
    if "fov_y" in camera:
        spec["fov_y"] = float(camera["fov_y"])
    return spec


def _default(value):
    """Render a value JSON cannot encode without importing its module."""
    return repr(value)


def _cell(value) -> str:
    """One table cell: scalars plainly, containers as compact JSON."""
    if isinstance(value, str):
        return value
    if value is None:
        return "—"
    if isinstance(value, (bool, int, float)):
        return json.dumps(value)
    if isinstance(value, (list, tuple, dict)):
        try:
            return json.dumps(value, sort_keys=True)
        except TypeError:
            return str(value)
    return str(value)


def main(run, **meta) -> int:
    """Run a workflow and print its capture as JSON on stdout."""
    cap = Capture(**meta)
    run(cap)
    print(json.dumps(cap.to_dict(), sort_keys=True))
    return 0
