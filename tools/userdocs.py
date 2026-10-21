#!/usr/bin/env python3
"""Generate the user documentation from the executable workflows.

Every page under ``docs/user/`` is rendered from a script in ``tools/workflows/``.
The script drives the real ``FreeCAD`` facade, so the code blocks on the page are
the code that ran and the values are what the run read back. That is the whole
point: the user documentation cannot drift from the product without the run
failing or ``--check`` reporting it.

Usage::

    python3 tools/userdocs.py             # regenerate docs/user/
    python3 tools/userdocs.py --check     # fail if any page is stale
    python3 tools/userdocs.py --list      # list the workflows
    python3 tools/userdocs.py --only toy-workbench

Adding a workflow is adding a file: ``docs/user/README.md`` is rebuilt from the
set of workflow scripts, so it is also the progress list.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
WORKFLOW_DIR = REPO / "tools" / "workflows"
DOC_DIR = REPO / "docs" / "user"
IMAGE_DIR = DOC_DIR / "images"
PYTHON_DIR = REPO / "python"

INDEX_PREAMBLE = """# User documentation

Generated documentation for the workflows FerroCAD implements **today**.

Each page is produced by running an ordinary Python script against the real
`FreeCAD` facade: the code blocks are the code that executed and the tables are
the values it read back. A workflow that is not implemented has no script, so it
has no page — this index is the progress list, not a backlog.
"""

INDEX_FOOTER = """## Regenerating

```sh
python3 tools/userdocs.py            # refresh every page
python3 tools/userdocs.py --check    # non-zero exit if a page is stale
```

The check is the part that matters: it is cheap, needs no GPU, and it is what
keeps the pages honest in review and in CI.

## What these pages are not

- **Not rendered screenshots.** A page shows the mesh the viewport would draw,
  summarised, but no pixels. Capturing a frame needs a GPU and an offscreen
  render path the shell does not expose yet; the `image` hook in
  `tools/workflows/_capture.py` is where that will attach.
- **Not the API reference.** For signatures see [`../python-api/`](../python-api/).
- **Not upstream parity.** Only what runs here is documented here; the gap is
  measured by [`../workbenches/conformance.md`](../workbenches/conformance.md).
"""


def discover() -> list[Path]:
    """Workflow scripts, in filename order (the numeric prefix is the read order)."""
    return sorted(
        p
        for p in WORKFLOW_DIR.glob("*.py")
        if not p.name.startswith("_")
    )


def run_workflow(path: Path) -> dict:
    """Execute one workflow in a fresh interpreter and return its capture."""
    env = dict(os.environ)
    env["PYTHONPATH"] = str(PYTHON_DIR)
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    # Deterministic iteration order for anything the facade exposes as a dict.
    env["PYTHONHASHSEED"] = "0"

    proc = subprocess.run(
        [sys.executable, str(path)],
        cwd=REPO,
        env=env,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"{path.name} failed (exit {proc.returncode})\n"
            f"--- stdout ---\n{proc.stdout}\n--- stderr ---\n{proc.stderr}"
        )

    lines = [line for line in proc.stdout.splitlines() if line.strip()]
    if not lines:
        raise RuntimeError(f"{path.name} produced no capture on stdout")
    try:
        return json.loads(lines[-1])
    except json.JSONDecodeError as exc:
        raise RuntimeError(
            f"{path.name} did not end with a JSON capture: {exc}\n{proc.stdout}"
        ) from exc


# ---------------------------------------------------------------------------
# Rendering
# ---------------------------------------------------------------------------
def _escape_cell(text: str) -> str:
    return str(text).replace("|", "\\|").replace("\n", "<br>")


def _table(headers, rows) -> list[str]:
    out = ["| " + " | ".join(_escape_cell(h) for h in headers) + " |"]
    out.append("| " + " | ".join("---" for _ in headers) + " |")
    for row in rows:
        out.append("| " + " | ".join(_escape_cell(c) for c in row) + " |")
    return out


def _collapse(lines: list[str]) -> str:
    """Join, dropping runs of blank lines (fenced content is a single element)."""
    out: list[str] = []
    for line in lines:
        if line == "" and out and out[-1] == "":
            continue
        out.append(line)
    return "\n".join(out).rstrip() + "\n"


def render_page(capture: dict, source: Path) -> str:
    """Render one workflow capture as a markdown page."""
    lines = [
        f"# {capture['title']}",
        "",
        capture["summary"],
        "",
        "> **Generated page.** `tools/userdocs.py` writes it from",
        f"> [`tools/workflows/{source.name}`](../../tools/workflows/{source.name}).",
        "> Edit the workflow, not this page; `python3 tools/userdocs.py --check`",
        "> fails when the two disagree.",
        "",
    ]

    if capture["requires"]:
        lines.append("**Requires:** " + "; ".join(capture["requires"]) + ".")
        lines.append("")

    step = 0
    for op in capture["ops"]:
        kind = op["kind"]
        if kind == "step":
            step += 1
            lines += ["", f"## {step}. {op['caption']}", ""]
        elif kind == "prose":
            lines += [op["text"], ""]
        elif kind == "note":
            lines += ["> " + op["text"].replace("\n", "\n> "), ""]
        elif kind == "code":
            lines += ["```python", op["source"], "```", ""]
        elif kind == "table":
            lines += _table(op["headers"], op["rows"])
            lines.append("")
        elif kind == "json":
            lines += [f"*{op['label']}*", "", "```json", op["text"], "```", ""]
        elif kind == "image":
            relative = f"images/{op['name']}"
            if (IMAGE_DIR / op["name"]).is_file():
                lines += [f"![{op['alt']}]({relative})", ""]
                if op.get("caption"):
                    lines += [f"*{op['caption']}*", ""]
            else:
                missing = op.get("caption") or op["alt"]
                lines += [
                    "> **No image captured in this environment.** " + missing,
                    "",
                ]
        else:  # pragma: no cover - a capture op the renderer does not know
            raise ValueError(f"unknown capture op {kind!r} in {source.name}")

    if capture["caveats"]:
        lines += ["", "## Notes", ""]
        lines += [f"- {c}" for c in capture["caveats"]]
        lines.append("")

    return _collapse(lines)


def render_index(captures: list[tuple[Path, dict]]) -> str:
    """Render docs/user/README.md from the set of workflows that exist."""
    lines = [INDEX_PREAMBLE]
    lines += [
        "| Workflow | What it shows | Needs |",
        "| --- | --- | --- |",
    ]
    for source, capture in captures:
        needs = "; ".join(capture["requires"]) or "—"
        lines.append(
            f"| [{capture['title']}]({capture['slug']}.md) "
            f"| {_escape_cell(capture['summary'])} "
            f"| {_escape_cell(needs)} |"
        )
    lines += ["", f"{len(captures)} workflows.", "", INDEX_FOOTER]
    return _collapse(lines)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------
def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="fail if a page is stale")
    parser.add_argument("--list", action="store_true", help="list the workflows")
    parser.add_argument("--only", metavar="SLUG", help="run one workflow by slug")
    args = parser.parse_args(argv)

    sources = discover()
    if args.list:
        for path in sources:
            print(path.name)
        return 0

    captures: list[tuple[Path, dict]] = []
    for path in sources:
        try:
            capture = run_workflow(path)
        except RuntimeError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1
        if args.only and capture["slug"] != args.only:
            continue
        captures.append((path, capture))

    if args.only and not captures:
        print(f"error: no workflow with slug {args.only!r}", file=sys.stderr)
        return 1
    if not captures:
        print("error: no workflows found", file=sys.stderr)
        return 1

    # `--only` still writes just that page; the index is only rebuilt from the
    # whole set, so it is not written in that mode.
    outputs: dict[Path, str] = {}
    for source, capture in captures:
        outputs[DOC_DIR / f"{capture['slug']}.md"] = render_page(capture, source)
    if not args.only:
        outputs[DOC_DIR / "README.md"] = render_index(captures)

    stale = []
    for target, text in outputs.items():
        current = target.read_text() if target.is_file() else None
        if current == text:
            continue
        stale.append(target)
        if args.check:
            print(f"stale: {target.relative_to(REPO)}", file=sys.stderr)
        else:
            target.write_text(text)
            print(f"wrote {target.relative_to(REPO)}")

    if args.check:
        if stale:
            print(
                "error: generated pages are out of date; run "
                "`python3 tools/userdocs.py`",
                file=sys.stderr,
            )
            return 1
        print(f"user docs are current ({len(outputs)} pages)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
