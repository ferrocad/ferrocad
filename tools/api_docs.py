#!/usr/bin/env python3
"""Generate the Python API reference from the live facade.

The reference is built by importing each module and reading its surface, so it
cannot list a function that no longer exists or miss one that was added. It is
deliberately a *signature and summary* reference: the behaviour behind each name
is documented by the user pages (``docs/user/``) and, for parity questions, by
the upstream FreeCAD reference.

Usage::

    python3 tools/api_docs.py            # regenerate docs/python-api/
    python3 tools/api_docs.py --check    # fail if a page is stale
    python3 tools/api_docs.py --list     # list the documented modules

Scope: the modules below are what FerroCAD ships. A name that upstream FreeCAD
has and this does not is simply absent — the conformance harness measures that
gap rather than this generated page pretending to.
"""

from __future__ import annotations

import argparse
import importlib
import inspect
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
DOC_DIR = REPO / "docs" / "python-api"
PYTHON_DIR = REPO / "python"

# (module, one-line role, extra note or None)
MODULES = [
    ("FreeCAD", "Application module: documents, objects, the base value types.", None),
    ("FreeCAD.Base", "The `Base` value types FreeCAD exposes as a submodule.", None),
    ("FreeCAD.Units", "Quantity parsing, units and number formatting.", None),
    ("FreeCAD.Console", "The console sink a workbench prints through.", None),
    (
        "FreeCADGui",
        "The GUI facade: commands, selection, view state, providers and overlays.",
        "`Selection` and `ViewState` are read-only snapshots pushed by the shell.",
    ),
    (
        "ferrocad_shell",
        "The host bridge the shell calls. Not upstream API: it is how FerroCAD's "
        "window (and `tools/userdocs.py`) drive the facade.",
        "Every entry point returns a JSON string.",
    ),
]

SKIP_NAMES = {"annotations"}
SKIP_TYPES = (type(sys),)  # imported modules are structure, not surface

INDEX_PREAMBLE = """# Python API reference

Generated from the live modules by `tools/api_docs.py`. Each page is a signature
and summary reference for what FerroCAD ships, not a tutorial: the workflows in
[`../user/`](../user/) show the same surface being used.

The product surface is the `FreeCAD` namespace, which deliberately mirrors
upstream, so the authoritative description of *behaviour* remains FreeCAD's own
documentation. What this section adds is the exact shape of the surface here.
"""

INDEX_FOOTER = """## Scope and honesty

- **Generated, not curated.** A page lists what the module actually exports when
  the generator runs. A missing name means missing here, not undocumented.
- **Signatures where Python exposes them.** Names implemented by the PyO3
  extension may not report a signature; those show as ``(...)``.
- **Not a parity claim.** Upstream has far more surface. The measured gap lives in
  [`../workbenches/conformance.md`](../workbenches/conformance.md).

## Regenerating

```sh
python3 tools/api_docs.py            # refresh every page
python3 tools/api_docs.py --check    # non-zero exit if a page is stale
```
"""


# ---------------------------------------------------------------------------
# Introspection
# ---------------------------------------------------------------------------
def _signature(obj) -> str | None:
    try:
        sig = inspect.signature(obj, eval_str=True)
    except (TypeError, ValueError, NameError):
        try:
            sig = inspect.signature(obj)
        except (TypeError, ValueError):
            return None
    params = list(sig.parameters.values())
    # PyO3 reports the `self` receiver; a reference table does not need it.
    if params and params[0].name == "self":
        sig = sig.replace(parameters=params[1:])
    return str(sig)


def _summary(obj) -> str:
    doc = inspect.getdoc(obj) or ""
    if not doc:
        return ""
    # First paragraph, on one line: enough to identify the name in a table.
    paragraph = doc.split("\n\n", 1)[0]
    return " ".join(paragraph.split())


def _public_names(module) -> list[str]:
    declared = getattr(module, "__all__", None)
    names = list(declared) if declared is not None else [
        n for n in dir(module) if not n.startswith("_")
    ]
    out = []
    for name in names:
        if name.startswith("_") or name in SKIP_NAMES:
            continue
        try:
            value = getattr(module, name)
        except Exception:  # noqa: BLE001 - a lazy attribute may raise
            continue
        if isinstance(value, SKIP_TYPES):
            continue
        out.append(name)
    return sorted(set(out))


def _members(obj) -> list[dict]:
    """Public members of a class or singleton, with their kind."""
    cls = obj if inspect.isclass(obj) else type(obj)
    rows = []
    for name in dir(cls):
        if name.startswith("_"):
            continue
        try:
            raw = inspect.getattr_static(cls, name)
        except AttributeError:
            continue
        if isinstance(raw, property):
            rows.append({"name": name, "kind": "property", "summary": _summary(raw)})
            continue
        try:
            value = getattr(cls, name)
        except Exception:  # noqa: BLE001 - a descriptor that raises is not surface
            continue
        if callable(value):
            sig = _signature(value)
            rows.append(
                {
                    "name": name,
                    "kind": "method",
                    "signature": sig,
                    "summary": _summary(value),
                }
            )
        else:
            rows.append({"name": name, "kind": "attribute", "summary": ""})
    return rows


def describe(module) -> dict:
    """Collect one module's surface into a JSON-able description."""
    functions, classes, objects, values = [], [], [], []

    for name in _public_names(module):
        value = getattr(module, name)
        if inspect.isclass(value):
            classes.append(
                {
                    "name": name,
                    "signature": _signature(value),
                    "summary": _summary(value),
                    "members": _members(value),
                }
            )
        elif inspect.isroutine(value):
            functions.append(
                {"name": name, "signature": _signature(value), "summary": _summary(value)}
            )
        elif inspect.ismodule(value):
            continue
        elif hasattr(value, "__dict__") and not isinstance(
            value, (str, int, float, bool, list, tuple, dict, type(None))
        ):
            objects.append(
                {
                    "name": name,
                    "type": type(value).__name__,
                    "summary": _summary(value),
                    "members": _members(value),
                }
            )
        else:
            # A plain value has no useful docstring; `inspect.getdoc` would return
            # its *type's* docstring (e.g. `str`'s), which reads as nonsense here.
            values.append({"name": name, "summary": "", "value": value})

    return {
        "module": module.__name__,
        "summary": _summary(module),
        "functions": sorted(functions, key=lambda d: d["name"]),
        "classes": sorted(classes, key=lambda d: d["name"]),
        "objects": sorted(objects, key=lambda d: d["name"]),
        "values": sorted(values, key=lambda d: d["name"]),
    }


# ---------------------------------------------------------------------------
# Rendering
# ---------------------------------------------------------------------------
def _escape(text: str) -> str:
    return str(text).replace("|", "\\|").replace("\n", " ")


def _table(rows, headers=("Name", "Summary")) -> list[str]:
    out = ["| " + " | ".join(headers) + " |", "| " + " | ".join("---" for _ in headers) + " |"]
    for row in rows:
        out.append("| " + " | ".join(_escape(c) for c in row) + " |")
    return out


def _member_table(members, title: str = "Member") -> list[str]:
    if not members:
        return []
    rows = []
    for member in members:
        label = member["name"]
        if member["kind"] == "method":
            label += member.get("signature") or "(...)"
        elif member["kind"] == "property":
            label = f"`{label}` (property)"
        elif member["kind"] == "attribute":
            label = f"`{label}` (attribute)"
        rows.append((label, member["summary"] or "—"))
    return _table(rows, headers=(title, "Summary"))


def _collapse(lines: list[str]) -> str:
    out: list[str] = []
    for line in lines:
        if line == "" and out and out[-1] == "":
            continue
        out.append(line)
    return "\n".join(out).rstrip() + "\n"


def render_module(description: dict, role: str, note: str | None) -> str:
    lines = [
        f"# `{description['module']}`",
        "",
        role,
        "",
        "> **Generated page.** `tools/api_docs.py` reads this surface from the",
        "> module itself. Do not edit it by hand.",
        "",
    ]
    if description["summary"]:
        lines += [description["summary"], ""]
    if note:
        lines += [f"_{note}_", ""]

    if description["functions"]:
        lines += ["## Functions", ""]
        lines += _table(
            [
                (f"`{f['name']}{f['signature'] or '(...)'}`", f["summary"] or "—")
                for f in description["functions"]
            ],
            headers=("Function", "Summary"),
        )
        lines.append("")

    for cls in description["classes"]:
        lines += [f"## Class `{cls['name']}`", ""]
        if cls["summary"]:
            lines += [cls["summary"], ""]
        table = _member_table(cls["members"], title="Member")
        if table:
            lines += table
            lines.append("")

    for obj in description["objects"]:
        lines += [f"## Object `{obj['name']}`", ""]
        if obj["summary"]:
            lines += [obj["summary"], ""]
        table = _member_table(obj["members"], title="Member")
        if table:
            lines += table
            lines.append("")

    if description["values"]:
        lines += ["## Values", ""]
        lines += _table(
            [
                (f"`{v['name']}`", v["summary"] or _value_text(v["value"]))
                for v in description["values"]
            ],
            headers=("Name", "Summary / value"),
        )
        lines.append("")

    return _collapse(lines)


def _value_text(value) -> str:
    try:
        text = repr(value)
    except Exception:  # noqa: BLE001 - a value may refuse to repr
        return "—"
    return text if len(text) <= 60 else text[:57] + "…"


def render_index(descriptions: list[tuple[dict, str, str | None]]) -> str:
    lines = [INDEX_PREAMBLE, "| Module | Public names | What it is |", "| --- | --- | --- |"]
    for description, role, _note in descriptions:
        count = sum(
            len(description[key])
            for key in ("functions", "classes", "objects", "values")
        )
        lines.append(
            f"| [`{description['module']}`]({_page_name(description['module'])}.md) "
            f"| {count} | {_escape(role)} |"
        )
    lines += ["", INDEX_FOOTER]
    return _collapse(lines)


def _page_name(module_name: str) -> str:
    return module_name


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------
def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="fail if a page is stale")
    parser.add_argument("--list", action="store_true", help="list the modules")
    args = parser.parse_args(argv)

    if args.list:
        for name, _role, _note in MODULES:
            print(name)
        return 0

    if str(PYTHON_DIR) not in sys.path:
        sys.path.insert(0, str(PYTHON_DIR))

    descriptions = []
    for name, role, note in MODULES:
        module = importlib.import_module(name)
        descriptions.append((describe(module), role, note))

    outputs: dict[Path, str] = {}
    for description, role, note in descriptions:
        outputs[DOC_DIR / f"{_page_name(description['module'])}.md"] = render_module(
            description, role, note
        )
    outputs[DOC_DIR / "README.md"] = render_index(descriptions)

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
                "error: generated API pages are out of date; run "
                "`python3 tools/api_docs.py`",
                file=sys.stderr,
            )
            return 1
        print(f"API reference is current ({len(outputs)} pages)")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
