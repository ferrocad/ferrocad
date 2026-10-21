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

    def image(self, name: str, alt: str, caption: str | None = None) -> None:
        """Refer to ``docs/user/images/<name>`` if it exists; say so if it does not."""
        self.ops.append(
            {"kind": "image", "name": name, "alt": alt, "caption": caption}
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
