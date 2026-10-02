#!/usr/bin/env python3
"""M3a surface inventory: parse FreeCAD's ``.pyi`` stubs into an API model.

FreeCAD keeps its Python-visible API contract in source-adjacent ``.pyi`` stubs
(320 files across ``src/App``, ``src/Base``, ``src/Gui``, ``src/Mod/*`` and a
handful of PySide overlays under ``src/Tools/typing``). Upstream is migrating
from the legacy ``<Class>Py.xml`` binding definitions to these stubs; on current
``main`` no ``*Py.xml`` remains, so the ``.pyi`` set is the authoritative target
surface (a pre-migration checkout is detected and reported). This tool walks that
tree, parses each stub with the standard-library ``ast`` module, and emits

* a machine-readable JSON model (the input for M3c PyO3 codegen), and
* a surface-size report grouped by source area (scoping the remaining parity work).

Why ``ast``: the stubs are plain Python syntax, so the stdlib parser handles the
full grammar (positional-only params, keyword-only params, ``X | None`` unions,
overloads, decorators) with zero dependencies and no drift from what upstream's
own ``generate_stubs.py`` consumes.

Usage::

    python3 tools/inventory.py --root ../freecad-upstream [--json api-model.json]
"""

from __future__ import annotations

import argparse
import ast
import json
import sys
from collections import defaultdict
from dataclasses import asdict, dataclass, field
from pathlib import Path

# ---------------------------------------------------------------------------
# Model
# ---------------------------------------------------------------------------


@dataclass
class Param:
    name: str
    kind: str          # posonly | pos | vararg | kwonly | kwarg
    annotation: str    # "" when unannotated
    default: bool
    default_value: str = ""   # unparse of the default expr, "" when absent


@dataclass
class Signature:
    params: list[Param] = field(default_factory=list)
    returns: str = ""


@dataclass
class Method:
    name: str
    signatures: list[Signature] = field(default_factory=list)  # >1 when overloaded
    decorators: list[str] = field(default_factory=list)
    is_classmethod: bool = False
    is_staticmethod: bool = False
    is_constmethod: bool = False
    is_overloaded: bool = False
    is_typing_only: bool = False


@dataclass
class Attribute:
    name: str
    annotation: str
    value: str = ""   # unparse of the RHS, or "" when absent


@dataclass
class Class:
    name: str
    bases: list[str] = field(default_factory=list)
    decorators: list[str] = field(default_factory=list)
    methods: list[Method] = field(default_factory=list)
    attributes: list[Attribute] = field(default_factory=list)


@dataclass
class Module:
    area: str            # App | Base | Gui | Mod.<wb> | PySide | Other
    path: str
    classes: list[Class] = field(default_factory=list)
    functions: list[Method] = field(default_factory=list)
    attributes: list[Attribute] = field(default_factory=list)


# ---------------------------------------------------------------------------
# Parsing
# ---------------------------------------------------------------------------


def _ann(node: ast.AST | None) -> str:
    return "" if node is None else ast.unparse(node)


def _value(node: ast.AST | None) -> str:
    return "" if node is None else ast.unparse(node)


def _decorator_name(dec: ast.expr) -> str:
    if isinstance(dec, ast.Name):
        return dec.id
    if isinstance(dec, ast.Attribute):
        return ast.unparse(dec)          # e.g. "Metadata.deprecated"
    if isinstance(dec, ast.Call):
        return _decorator_name(dec.func)  # e.g. "export", "deprecated"
    return ast.unparse(dec)


def _base_name(node: ast.expr) -> str:
    if isinstance(node, ast.Name):
        return node.id
    return ast.unparse(node)


def _walk(body: list[ast.stmt]):
    """Yield statements, recursing into ``if TYPE_CHECKING:`` blocks."""
    for node in body:
        if isinstance(node, ast.If):
            yield from _walk(node.body)
            yield from _walk(node.orelse)
        else:
            yield node


def _signature(node: ast.FunctionDef | ast.AsyncFunctionDef) -> Signature:
    a = node.args
    params: list[Param] = []

    # Positional params = posonlyargs + args; `defaults` aligns to the trailing
    # entries of that combined list (CPython stores pos-only defaults here too).
    positional = list(a.posonlyargs) + list(a.args)
    n_posonly = len(a.posonlyargs)
    n_defaults = len(a.defaults)
    n_no_default = len(positional) - n_defaults
    for i, p in enumerate(positional):
        kind = "posonly" if i < n_posonly else "pos"
        default = i >= n_no_default
        default_value = _value(a.defaults[i - n_no_default]) if default else ""
        params.append(Param(p.arg, kind, _ann(p.annotation), default, default_value))

    if a.vararg is not None:
        params.append(Param(a.vararg.arg, "vararg", _ann(a.vararg.annotation), False))

    for i, p in enumerate(a.kwonlyargs):
        d = a.kw_defaults[i]
        params.append(Param(p.arg, "kwonly", _ann(p.annotation), d is not None, _value(d)))

    if a.kwarg is not None:
        params.append(Param(a.kwarg.arg, "kwarg", _ann(a.kwarg.annotation), False))

    # Drop the implicit receiver so the model records the *callable* signature
    # (upstream stubs spell out `self`/`cls`, but it is not part of the API).
    if params and params[0].name in ("self", "cls") and params[0].kind in ("pos", "posonly"):
        params = params[1:]

    return Signature(params=params, returns=_ann(node.returns))


def _merge_decorators(existing: list[str], new: list[str]) -> list[str]:
    out = list(existing)
    for d in new:
        if d not in out:
            out.append(d)
    return out


def _collect_methods(body: list[ast.stmt]) -> list[Method]:
    by_name: dict[str, Method] = {}
    order: list[str] = []

    for node in _walk(body):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            name = node.name
            decos = [_decorator_name(d) for d in node.decorator_list]
            if name not in by_name:
                by_name[name] = Method(name=name)
                order.append(name)
            m = by_name[name]
            m.signatures.append(_signature(node))
            m.decorators = _merge_decorators(m.decorators, decos)
            if "overload" in decos:
                m.is_overloaded = True
            if "classmethod" in decos:
                m.is_classmethod = True
            if "staticmethod" in decos:
                m.is_staticmethod = True
            if "constmethod" in decos:
                m.is_constmethod = True
            if "typing_only" in decos:
                m.is_typing_only = True

    return [by_name[n] for n in order]


def _collect_attributes(body: list[ast.stmt]) -> list[Attribute]:
    attrs: list[Attribute] = []
    for node in _walk(body):
        if isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            attrs.append(Attribute(node.target.id, _ann(node.annotation), _value(node.value)))
        elif isinstance(node, ast.Assign):
            for t in node.targets:
                if isinstance(t, ast.Name):
                    attrs.append(Attribute(t.id, "", _value(node.value)))
    return attrs


def _class_from(node: ast.ClassDef) -> Class:
    return Class(
        name=node.name,
        bases=[_base_name(b) for b in node.bases],
        decorators=[_decorator_name(d) for d in node.decorator_list],
        methods=_collect_methods(node.body),
        attributes=_collect_attributes(node.body),
    )


def parse_stub(path: Path, area: str) -> Module:
    """Parse one ``.pyi`` file into a ``Module``."""
    source = path.read_text(encoding="utf-8")
    # type_comments is left off: annotations live on the AST nodes, and some
    # upstream `# type:` comments are malformed and would raise a SyntaxError.
    tree = ast.parse(source, filename=str(path))

    mod = Module(area=area, path=str(path))
    for node in _walk(tree.body):
        if isinstance(node, ast.ClassDef):
            mod.classes.append(_class_from(node))
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            decos = [_decorator_name(d) for d in node.decorator_list]
            m = Method(name=node.name, signatures=[_signature(node)], decorators=decos)
            m.is_overloaded = "overload" in decos
            m.is_classmethod = "classmethod" in decos
            m.is_staticmethod = "staticmethod" in decos
            m.is_constmethod = "constmethod" in decos
            m.is_typing_only = "typing_only" in decos
            mod.functions.append(m)
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            mod.attributes.append(Attribute(node.target.id, _ann(node.annotation), _value(node.value)))
    return mod


# ---------------------------------------------------------------------------
# Discovery
# ---------------------------------------------------------------------------


def classify_area(rel_path: Path) -> str:
    """Map a stub path (relative to ``--root``) to a surface area."""
    parts = rel_path.parts
    text = str(rel_path)
    if "overlays/PySide" in text:
        return "PySide"
    if parts and parts[0] == "App":
        return "App"
    if parts and parts[0] == "Base":
        return "Base"
    if parts and parts[0] == "Gui":
        return "Gui"
    if parts and parts[0] == "Mod":
        return f"Mod.{parts[1]}" if len(parts) >= 3 else "Mod"
    return "Other"


def discover(root: Path) -> list[Module]:
    """Find and parse every ``.pyi`` stub under ``root/src``.

    Upstream is mid-migration from the legacy ``<Class>Py.xml`` binding
    definitions to ``.pyi`` stubs: on current ``main`` the XML files are gone and
    every ``*PyImp.cpp`` has a matching ``.pyi``. When pointed at an older tree
    that still carries ``*Py.xml``, classes defined only there would be invisible
    to a ``.pyi``-only walk, so warn about it.
    """
    src = root / "src"
    if not src.is_dir():
        raise SystemExit(f"no src/ directory under {root}")

    legacy = sorted(src.rglob("*Py.xml"))
    if legacy:
        unstubbed = [p for p in legacy if not p.with_suffix(".pyi").exists()]
        print(
            f"warning: found {len(legacy)} legacy *Py.xml binding file(s)"
            f" ({len(unstubbed)} without a .pyi) - this checkout predates the"
            " XML->pyi migration and the inventory may be incomplete",
            file=sys.stderr,
        )

    modules: list[Module] = []
    errors: list[str] = []
    for path in sorted(src.rglob("*.pyi")):
        rel = path.relative_to(src)
        try:
            modules.append(parse_stub(path, classify_area(rel)))
        except SyntaxError as exc:
            errors.append(f"{path}: {exc}")
    if errors:
        print("skipped (syntax error):", file=sys.stderr)
        for e in errors:
            print(f"  {e}", file=sys.stderr)
    return modules


# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------


def _signature_count(methods: list[Method]) -> int:
    return sum(len(m.signatures) for m in methods)


def summarize(modules: list[Module]) -> dict:
    areas: dict[str, dict[str, int]] = defaultdict(
        lambda: {"files": 0, "classes": 0, "methods": 0, "signatures": 0,
                 "overloaded": 0, "attributes": 0, "functions": 0}
    )
    totals = {
        "files": 0, "classes": 0, "methods": 0, "signatures": 0,
        "overloaded": 0, "attributes": 0, "functions": 0, "module_attributes": 0,
    }

    for mod in modules:
        a = areas[mod.area]
        a["files"] += 1
        a["classes"] += len(mod.classes)
        for c in mod.classes:
            a["methods"] += len(c.methods)
            a["signatures"] += _signature_count(c.methods)
            a["overloaded"] += sum(1 for m in c.methods if m.is_overloaded)
            a["attributes"] += len(c.attributes)
        a["functions"] += len(mod.functions)
        for attr in mod.attributes:
            a["attributes"] += 1
            totals["module_attributes"] += 1

    for a in areas.values():
        for key in totals:
            if key in a:
                totals[key] += a[key]

    return {"totals": totals, "areas": dict(sorted(areas.items()))}


def render_report(root: Path, modules: list[Module], summary: dict) -> str:
    t = summary["totals"]
    lines: list[str] = []
    lines.append("FreeCAD API surface inventory (M3a)")
    lines.append("=" * 44)
    lines.append(f"source: {root}")
    lines.append(f"stub files: {t['files']}")
    lines.append("")
    lines.append("Totals")
    lines.append("-" * 44)
    lines.append(f"  classes            : {t['classes']}")
    lines.append(f"  methods (unique)   : {t['methods']}  ({t['overloaded']} overloaded)")
    lines.append(f"  method signatures  : {t['signatures']}")
    lines.append(f"  class attributes   : {t['attributes']}")
    lines.append(f"  module functions   : {t['functions']}")
    lines.append(f"  module attributes  : {t['module_attributes']}")
    lines.append("")
    lines.append("By area")
    lines.append("-" * 44)
    header = f"  {'area':<14} {'files':>5} {'cls':>5} {'meth':>5} {'sigs':>6} {'ovl':>4} {'attr':>5} {'fn':>4}"
    lines.append(header)
    for area, a in summary["areas"].items():
        lines.append(
            f"  {area:<14} {a['files']:>5} {a['classes']:>5} {a['methods']:>5} "
            f"{a['signatures']:>6} {a['overloaded']:>4} {a['attributes']:>5} {a['functions']:>4}"
        )
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Inventory FreeCAD's .pyi stubs.")
    parser.add_argument("--root", required=True, help="path to the freecad-upstream checkout")
    parser.add_argument("--json", help="write the API model as JSON to this path")
    args = parser.parse_args(argv)

    root = Path(args.root).resolve()
    modules = discover(root)
    summary = summarize(modules)

    print(render_report(root, modules, summary))

    if args.json:
        model = {
            "generated_by": "tools/inventory.py (M3a)",
            "source_root": str(root),
            "summary": summary["totals"],
            "modules": [asdict(m) for m in modules],
        }
        Path(args.json).write_text(json.dumps(model, indent=2), encoding="utf-8")
        print(f"\nwrote API model: {args.json}")

    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
