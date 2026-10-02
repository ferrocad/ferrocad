#!/usr/bin/env python3
"""M3c codegen: emit PyO3 skeleton bindings from the M3a API model.

Reads the FreeCAD ``.pyi`` stubs (via ``inventory``) and generates a Rust
extension crate whose ``#[pyclass]``/``#[pymethods]`` surface mirrors a chosen
slice. Bodies are ``todo!()`` stubs — this is the **skeleton** half of decision
"generate skeleton bindings + hand-written behaviour glue"; behaviour lives in
``ferrocad_core`` and is filled in separately.

The generator deliberately keeps the Rust *simple and always-compiling*:

* every parameter/return type maps to a small set (``String``/``i64``/``f64``/
  ``bool``/``Vec<PyObject>``/``PyObject``); anything unknown becomes ``PyObject``,
* ``#[pyo3(signature = (...))]`` is emitted only when every default is a plain
  literal for a mapped primitive; otherwise the method is all-required,
* attribute getters return a type-appropriate zero value, setters are no-ops,
* Rust-keyword parameter names (``type``, ``match``, …) are suffixed with ``_``.

Usage::

    python3 tools/codegen.py --root ../freecad-upstream --out crates/ferrocad_gen/src/lib.rs
"""

from __future__ import annotations

import argparse
import ast
import sys
from pathlib import Path

import inventory

DEFAULT_SLICE = ["Document", "DocumentObject", "PropertyContainer"]
DEFAULT_AREA = "App"

# Rust keywords that cannot be used as parameter/identifier names.
RUST_KEYWORDS = frozenset(
    """
    as break const continue crate dyn else enum extern false fn for if impl in
    let loop match mod move mut pub ref return self Self static struct super
    trait true type unsafe use where while async await box abstract become do
    final macro override priv typeof unsized virtual yield try union
    """.split()
)


def sanitize(name: str) -> str:
    """Return a valid Rust identifier for a Python parameter name."""
    return f"{name}_" if name in RUST_KEYWORDS else name


def map_annotation(ann: str) -> str:
    """Map a Python stub annotation to a Rust type (skeleton-safe)."""
    a = ann.strip()
    while a.startswith("Final[") and a.endswith("]"):
        a = a[len("Final["):-1].strip()
    while a.startswith("Optional[") and a.endswith("]"):
        a = a[len("Optional["):-1].strip()

    if a == "str":
        return "String"
    if a == "int":
        return "i64"
    if a == "float":
        return "f64"
    if a == "bool":
        return "bool"
    if (
        a in ("list", "tuple")
        or a.startswith("list[")
        or a.startswith("tuple[")
        or a.startswith("List[")
        or a.startswith("Tuple[")
    ):
        return "Vec<PyObject>"
    # unions, Union[...], named types, Any, object, bytearray, ...
    return "PyObject"


def map_return(ann: str) -> str:
    a = ann.strip()
    if a == "None":
        return "()"
    return map_annotation(a)


def map_param_type(ann: str) -> str:
    """Like ``map_annotation`` but ``str`` -> ``&str`` so string defaults are
    plain Rust literals (introspectable as ``''``/``'Base'`` instead of ``...``)."""
    rt = map_annotation(ann)
    return "&str" if rt == "String" else rt


def zero_value(rust_type: str, py: str) -> str:
    """Return a zero-value expression for a mapped Rust type."""
    return {
        "String": "String::new()",
        "i64": "0",
        "f64": "0.0",
        "bool": "false",
        "Vec<PyObject>": "Vec::new()",
        "PyObject": f"{py}.None()",
    }[rust_type]


def _rust_str(value: str) -> str:
    """Return a Rust string literal for a Python string value."""
    out = ['"']
    for ch in value:
        if ch == "\\":
            out.append("\\\\")
        elif ch == '"':
            out.append('\\"')
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\t":
            out.append("\\t")
        elif ch == "\r":
            out.append("\\r")
        elif ord(ch) < 0x20:
            out.append(f"\\u{{{ord(ch):x}}}")
        else:
            out.append(ch)
    out.append('"')
    return "".join(out)


def default_literal(annotation: str, default_value: str) -> str | None:
    """Map a ``.pyi`` default to a Rust expression, or ``None`` if unmappable."""
    rt = map_param_type(annotation)
    if rt == "&str":
        if default_value == "...":
            return '""'
        try:
            actual = ast.literal_eval(default_value)
        except Exception:
            return None
        if not isinstance(actual, str):
            return None
        return _rust_str(actual)
    if rt == "i64":
        if default_value == "...":
            return "0"
        try:
            actual = ast.literal_eval(default_value)
        except Exception:
            return None
        return str(actual) if isinstance(actual, int) else None
    if rt == "f64":
        if default_value == "...":
            return "0.0"
        try:
            actual = ast.literal_eval(default_value)
        except Exception:
            return None
        if isinstance(actual, float):
            return str(actual)
        if isinstance(actual, int):
            return f"{actual}.0"
        return None
    if rt == "bool":
        if default_value == "...":
            return "false"
        try:
            actual = ast.literal_eval(default_value)
        except Exception:
            return None
        if isinstance(actual, bool):
            return "true" if actual else "false"
        return None
    return None


def render_signature(method: inventory.Method) -> str | None:
    """Return ``#[pyo3(signature = (...))]`` or ``None`` when it can't be emitted."""
    sig = method.signatures[-1]  # the non-overload implementation signature
    parts: list[str] = []
    seen_kwonly = False
    for p in sig.params:
        if p.kind in ("vararg", "kwarg"):
            return None  # too complex for a skeleton; make all params required
        if p.kind == "kwonly" and not seen_kwonly:
            parts.append("*")
            seen_kwonly = True
        if p.default:
            lit = default_literal(p.annotation, p.default_value)
            if lit is None:
                return None
            parts.append(f"{sanitize(p.name)} = {lit}")
        else:
            parts.append(sanitize(p.name))
    return "#[pyo3(signature = (" + ", ".join(parts) + "))]"


def render_param(p: inventory.Param) -> str:
    """Render one method parameter (Rust name + mapped type)."""
    if p.kind == "vararg":
        return f"{sanitize(p.name)}: &Bound<'_, PyTuple>"
    if p.kind == "kwarg":
        return f"{sanitize(p.name)}: Option<&Bound<'_, PyDict>>"
    return f"{sanitize(p.name)}: {map_param_type(p.annotation)}"


def render_method(method: inventory.Method, class_name: str) -> list[str]:
    out: list[str] = []
    sig = method.signatures[-1]

    params = [render_param(p) for p in sig.params]
    returns = map_return(sig.returns)

    if method.is_staticmethod:
        out.append("    #[staticmethod]")
        fn_params = params
    elif method.is_classmethod:
        out.append("    #[classmethod]")
        fn_params = ["_cls: &Bound<'_, PyType>"] + params
    else:
        fn_params = ["&self"] + params

    signature = render_signature(method)
    if signature is not None:
        out.append(f"    {signature}")

    joined = ", ".join(fn_params)
    out.append(
        f"    fn {sanitize(method.name)}({joined}) -> {returns} "
        f'{{ todo!("{class_name}.{method.name}") }}'
    )
    return out


def render_attribute(attr: inventory.Attribute, class_name: str) -> list[str]:
    name = sanitize(attr.name)
    rust_type = map_annotation(attr.annotation)
    read_only = attr.annotation.strip().startswith("Final")

    out: list[str] = []
    if rust_type == "PyObject":
        out.append(f"    #[getter]")
        out.append(f"    fn {name}(&self, py: Python<'_>) -> PyObject {{ {zero_value('PyObject', 'py')} }}")
    else:
        out.append("    #[getter]")
        out.append(f"    fn {name}(&self) -> {rust_type} {{ {zero_value(rust_type, 'py')} }}")

    if not read_only:
        out.append("    #[setter]")
        out.append(f"    fn set_{name}(&mut self, value: {rust_type}) {{ let _ = value; }}")
    return out


def render_class(cls: inventory.Class) -> list[str]:
    out: list[str] = []
    out.append(f"#[pyclass(name = \"{cls.name}\", module = \"ferrocad_gen\")]")
    out.append(f"struct {cls.name};")
    out.append("")
    out.append("#[pymethods]")
    out.append(f"impl {cls.name} {{")
    for attr in cls.attributes:
        out.extend(render_attribute(attr, cls.name))
    for method in cls.methods:
        out.extend(render_method(method, cls.name))
    out.append("}")
    return out


def render_module(classes: list[inventory.Class]) -> str:
    lines: list[str] = []
    lines.append("//! GENERATED by `tools/codegen.py` — do not edit by hand.")
    lines.append("//! M3c skeleton bindings for the FreeCAD App document/object surface.")
    lines.append("//! Method bodies are `todo!()`; behaviour is filled in by hand-written glue.")
    lines.append("")
    lines.append("#![allow(non_snake_case, unused_variables, unused_imports, clippy::all)]")
    lines.append("")
    lines.append("use pyo3::prelude::*;")
    lines.append("use pyo3::types::{PyAnyMethods, PyDict, PyTuple, PyType};")
    lines.append("")
    for cls in classes:
        lines.extend(render_class(cls))
        lines.append("")
    lines.append("#[pymodule]")
    lines.append("fn ferrocad_gen(m: &Bound<'_, PyModule>) -> PyResult<()> {")
    for cls in classes:
        lines.append(f"    m.add_class::<{cls.name}>()?;")
    lines.append("    Ok(())")
    lines.append("}")
    return "\n".join(lines) + "\n"


def select(modules: list[inventory.Module], area: str, names: list[str]) -> list[inventory.Class]:
    chosen: dict[str, inventory.Class] = {}
    for mod in modules:
        if mod.area != area:
            continue
        for cls in mod.classes:
            if cls.name in names:
                chosen[cls.name] = cls
    missing = [n for n in names if n not in chosen]
    if missing:
        print(f"warning: classes not found in area '{area}': {missing}", file=sys.stderr)
    return [chosen[n] for n in names if n in chosen]


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Generate PyO3 skeleton bindings.")
    parser.add_argument("--root", required=True, help="path to the freecad-upstream checkout")
    parser.add_argument("--out", default="crates/ferrocad_gen/src/lib.rs", help="output Rust file")
    parser.add_argument("--area", default=DEFAULT_AREA, help="stub area to slice (default: App)")
    parser.add_argument(
        "--class",
        dest="classes",
        action="append",
        help="class name to include (repeatable; default: Document, DocumentObject, PropertyContainer)",
    )
    args = parser.parse_args(argv)

    names = args.classes or DEFAULT_SLICE
    modules = inventory.discover(Path(args.root).resolve())
    classes = select(modules, args.area, names)
    if not classes:
        print("error: no classes selected", file=sys.stderr)
        return 1

    out_path = Path(args.out)
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(render_module(classes), encoding="utf-8")
    print(f"wrote {out_path} ({len(classes)} classes: {', '.join(c.name for c in classes)})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
