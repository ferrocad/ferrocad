#!/usr/bin/env python3
"""H1 Part/OCCT API audit: pin what ``Part`` exposes against upstream.

H1 is "finish Part/OCCT". Prose lists rot; this tool derives the same inventory
from the Rust bindings and from a sibling ``freecad-upstream`` checkout, then
compares both with ``contracts/part-api.json``. The contract is the source of
truth and every number in it is counted, never hard-coded here.

What is derived
---------------

``ours`` — ``crates/ferrocad_part_py/src/*.rs``:

==========================  ==================================================
``functions.registered``    ``<module>.add_function(wrap_pyfunction!(...))``,
                            qualified by the receiving module (``Part``,
                            ``Part.Precision``)
``functions.defined``       every ``#[pyfunction]`` declaration, registered or
                            not (``defined_not_registered`` names the latter)
``classes``                 ``#[pyclass(name = "...", module = "...")]``
``object_types``            registered object types from both
                            ``crates/ferrocad_part/src`` and ``ferrocad_part_py``:
                            ``*_TYPE`` constants and literal ids passed to
                            ``object_registry::register`` / a local ``register``
                            helper (loop tables expanded); property types and
                            FaceMaker selectors are excluded
``gap_markers``             literal ``not implemented yet``, ``not available in
                            this slice``, ``unsupported(`` and ``NotImplemented``
``methods``                 per-Python-class member names: ``fn`` items, with
                            ``#[getter]``/``#[setter]`` folded to the property
``methods_heuristic``       ``fn`` items inside ``#[pymethods]`` blocks; a text
                            pass only, and deliberately not compared
==========================  ==================================================

``upstream`` — ``<root>/src/Mod/Part/App``:

- module functions from ``AppPartPy.cpp`` (``add_varargs_method`` and friends),
  split by their ``Py::ExtensionModule`` class;
- Python class names from ``Base::Interpreter().addType(..., "Name")`` in
  ``AppPart.cpp``, which is more reliable than a file-name guess;
- **member names per class from the ``.pyi`` stubs** (``@export(PythonName=…)``
  plus the class attributes and ``def`` items), which is the method-level
  surface the C++ sources only generate;
- ``.pyi``/``*PyImp.cpp`` stub names, counted for the text-pass check but not
  used in the class diff (the stub is named after the C++ class, so
  ``TopoShape.pyi`` is Python's ``Part.Shape``);
- object types from ``PROPERTY_SOURCE*(Part::X, ...)``; the
  ``TYPESYSTEM_SOURCE*`` names are inventoried separately because they are
  mostly geometry/property types, not document objects. This is a
  registered-type inventory, so it includes base/abstract document classes and
  ``App::Extension`` classes too, not only addObject-able objects.
  ``PART_*_TYPE`` macros are counted too and are absent in this checkout.

Usage::

    python3 tools/part_api_audit.py              # report what is missing
    python3 tools/part_api_audit.py --check      # exit 1 on drift (CI)
    python3 tools/part_api_audit.py --write      # re-pin the contract
    python3 tools/part_api_audit.py --json       # machine-readable output

The checkout is ``--root``, then ``$FERROCAD_UPSTREAM``, then
``../freecad-upstream`` beside the repo. ``--root`` may point at the checkout or
directly at ``src/Mod/Part/App``.

CI is optional: the ``geometry`` job runs ``--check`` only when a sibling
``../freecad-upstream`` exists. A missing checkout skips cleanly with exit 0,
because there is nothing to compare against; a missing contract, or a partial
checkout, is a setup error (exit 2).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from collections import Counter
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]  # ferrocad project root
DEFAULT_ROOT = REPO.parent / "freecad-upstream"
DEFAULT_CONTRACT = REPO / "contracts" / "part-api.json"
OURS_BINDING_DIR = REPO / "crates" / "ferrocad_part_py" / "src"
OURS_PART_DIR = REPO / "crates" / "ferrocad_part" / "src"
OURS_SOURCE = "crates/ferrocad_part_py/src"
OURS_OBJECT_SOURCES = ("crates/ferrocad_part/src", OURS_SOURCE)
UPSTREAM_SOURCE = "src/Mod/Part/App"
UPSTREAM_URL = "https://github.com/FreeCAD/FreeCAD"

#: The honest gap markers that this slice asked for, in a fixed order.
GAP_MARKERS = (
    ("not_implemented_yet", "not implemented yet"),
    ("not_available_in_this_slice", "not available in this slice"),
    ("unsupported", "unsupported("),
    ("not_implemented", "NotImplemented"),
)

# --- our Rust sources ------------------------------------------------------
PYFUNCTION = re.compile(r"#\[pyfunction(?:\(([^)]*)\))?\]")
PYFUNCTION_NAME = re.compile(r'\bname\s*=\s*"([^"]+)"')
FN_DECL = re.compile(r"\bfn\s+([A-Za-z_]\w*)")
PYCLASS = re.compile(r"#\[pyclass\((.*?)\)\]", re.S)
ATTR_NAME = re.compile(r'\bname\s*=\s*"([^"]+)"')
ATTR_MODULE = re.compile(r'\bmodule\s*=\s*"([^"]+)"')
ATTR_EXTENDS = re.compile(r'\bextends\s*=\s*([A-Za-z_]\w*)')
ADD_FUNCTION = re.compile(
    r"([A-Za-z_]\w*)\s*\.add_function\s*\(\s*wrap_pyfunction!\s*\(\s*([A-Za-z_]\w*)"
)
#: ``let precision = PyModule::new(py, "Part.Precision")?;``
PYMODULE_NEW = re.compile(
    r'\blet\s+([A-Za-z_]\w*)\s*=\s*PyModule::new\([^,]+,\s*"([^"]+)"\)'
)
TYPE_CONST = re.compile(r'\bconst\s+([A-Z][A-Z0-9_]*)\s*:\s*&str\s*=\s*"([^"]+)"')
OBJECT_REGISTER = re.compile(
    r'object_registry::register\s*\(\s*([A-Z][A-Z0-9_]*|"[^"]*")\s*,'
)
#: A local ``register(...)`` helper (the crate may wrap ``object_registry``).
LOCAL_REGISTER = re.compile(
    r'(?<![\w:.])register\s*\(\s*([A-Z][A-Z0-9_]*|"[^"]*")\s*,'
)
#: ``for (type_id, label) in [(FUSE_TYPE, "Fuse"), ...] {``, expanded only when
#: the loop variable is passed to a register call in the body.
LOOP_TABLE = re.compile(r"for\s*\(\s*([A-Za-z_]\w*)\s*,[^)]*\)\s*in\s*\[", re.S)
TABLE_ENTRY = re.compile(r'\(\s*([A-Z][A-Z0-9_]*|"[^"]*")\s*,')
PYMETHODS = re.compile(r"#\[pymethods\]")
METHOD_FN = re.compile(r"(?m)^\s+(?:pub(?:\([^)]*\))?\s+)?fn\s+[A-Za-z_]\w*")

# --- upstream C++ sources --------------------------------------------------
UPSTREAM_CLASS = re.compile(r"class\s+(\w+)\s*:\s*public\s+Py::ExtensionModule<")
UPSTREAM_MODULE = re.compile(r'Py::ExtensionModule<\w+>\s*\(\s*"([^"]+)"\s*\)')
UPSTREAM_METHOD = re.compile(r'add_(varargs|keyword|noargs)_method\s*\(\s*"([^"]+)"')
ADD_TYPE = re.compile(
    r'addType\s*\(\s*&([\w:]+?)Py\s*::Type\s*,\s*(\w+)\s*,\s*"([^"]+)"\s*\)',
    re.S,
)
GET_ATTR = re.compile(r'PyObject\*\s*(\w+)\s*\(\s*(\w+)\.getAttr\("([^"]+)"\)')
PROPERTY_SOURCE = re.compile(r"PROPERTY_SOURCE[A-Z_]*\s*\(\s*([A-Za-z_][\w:]*)\s*,")
TYPESYSTEM_SOURCE = re.compile(r"TYPESYSTEM_SOURCE[A-Z_]*\s*\(\s*([A-Za-z_][\w:]*)\s*,")
PART_TYPE_MACRO = re.compile(r"\bPART_[A-Z0-9_]+_TYPE\b")

# --- method inventories ----------------------------------------------------
# Our classes: `#[pyclass(...)] struct <RustType>` names the Python class, and
# each `#[pymethods] impl <RustType>` block declares its members.
PYCLASS_STRUCT = re.compile(
    r"#\[pyclass\((.*?)\)\]\s*(?:pub(?:\([^)]*\))?\s+)?struct\s+([A-Za-z_]\w*)",
    re.S,
)
PYMETHODS_IMPL = re.compile(r"#\[pymethods\]\s*impl\s+([A-Za-z_]\w*)", re.S)
RUST_FN = re.compile(
    r"(?m)^\s+(?:pub(?:\([^)]*\))?\s+)?fn\s+([A-Za-z_]\w*)(?:\s*<[^>]*>)?\s*\("
)
RUST_GETTER = re.compile(
    r"#\[getter\]\s*(?:\n\s*)*(?:pub(?:\([^)]*\))?\s+)?fn\s+([A-Za-z_]\w*)(?:\s*<[^>]*>)?\s*\("
)
RUST_SETTER = re.compile(
    r"#\[setter\]\s*(?:\n\s*)*(?:pub(?:\([^)]*\))?\s+)?fn\s+set_([A-Za-z_]\w*)(?:\s*<[^>]*>)?\s*\("
)

# Upstream stubs: `@export(PythonName="Part.X", ...)` then `class _:`. Members
# are the class's own attributes and `def` items (four-space indentation).
PYI_EXPORT_CLASS = re.compile(r"@export\((.*?)\)\s*class\s+([A-Za-z_]\w*)\s*\(", re.S)
PYI_PYTHON_NAME = re.compile(r'PythonName\s*=\s*"([^"]+)"')
PYI_METHOD = re.compile(r"^    (?:async\s+)?def\s+([A-Za-z_]\w*)\s*\(", re.M)
PYI_ATTRIBUTE = re.compile(r"^    ([A-Za-z_]\w*)\s*:\s*\S", re.M)


# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
def checkout_path(explicit: str | None = None) -> Path:
    """Resolve the checkout: CLI, then ``$FERROCAD_UPSTREAM``, then the sibling."""
    if explicit:
        return Path(explicit).expanduser().resolve()
    env = os.environ.get("FERROCAD_UPSTREAM")
    if env:
        return Path(env).expanduser().resolve()
    return DEFAULT_ROOT.resolve()


def upstream_app(root: Path) -> Path | None:
    """The Part/App directory, accepting the checkout root or the App dir."""
    for candidate in (
        root / UPSTREAM_SOURCE,
        root / "Mod" / "Part" / "App",
        root,
    ):
        if (candidate / "AppPart.cpp").is_file() and (
            candidate / "AppPartPy.cpp"
        ).is_file():
            return candidate
    return None


def _read(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace")


def _revision(root: Path) -> str:
    try:
        out = subprocess.run(
            ["git", "--no-pager", "-C", str(root), "rev-parse", "--short", "HEAD"],
            capture_output=True,
            text=True,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return "unknown"
    return out.stdout.strip() or "unknown"


# ---------------------------------------------------------------------------
# Our surface
# ---------------------------------------------------------------------------
def _pyfunctions(files: list[Path]) -> dict[str, str]:
    """Rust ``fn`` name -> Python name for every ``#[pyfunction]``."""
    found: dict[str, str] = {}
    for path in files:
        text = _read(path)
        for match in PYFUNCTION.finditer(text):
            decl = FN_DECL.search(text, match.end(), match.end() + 1000)
            if decl is None:
                continue
            attrs = match.group(1) or ""
            named = PYFUNCTION_NAME.search(attrs)
            found[decl.group(1)] = named.group(1) if named else decl.group(1)
    return found


def _receiver_modules(text: str) -> dict[str, str]:
    """Map the local ``add_function`` receiver to its Python module name."""
    modules = {"m": "Part", "module": "Part"}
    for match in PYMODULE_NEW.finditer(text):
        modules[match.group(1)] = match.group(2)
    return modules


def _matching(text: str, start: int, opener: str, closer: str) -> int:
    """Index of the delimiter that closes the one at ``start``, or -1."""
    depth = 0
    for index in range(start, len(text)):
        char = text[index]
        if char == opener:
            depth += 1
        elif char == closer:
            depth -= 1
            if depth == 0:
                return index
    return -1


def _registration_tokens(text: str) -> list[str]:
    """Type tokens passed to an object registration, including loop tables.

    A direct call is ``object_registry::register(BOX_TYPE, ...)`` or a local
    ``register(BOX_TYPE, ...)`` helper. A table such as ``for (type_id, label)
    in [(FUSE_TYPE, "Fuse"), ...]`` is expanded only when the loop variable is
    the first argument of a register call in the body, so test tables that call
    ``add_object`` do not count.
    """
    tokens = [match.group(1) for match in OBJECT_REGISTER.finditer(text)]
    tokens.extend(match.group(1) for match in LOCAL_REGISTER.finditer(text))

    for head in LOOP_TABLE.finditer(text):
        variable = head.group(1)
        array_open = head.end() - 1
        array_close = _matching(text, array_open, "[", "]")
        if array_close < 0:
            continue
        body_open = text.find("{", array_close)
        if body_open < 0:
            continue
        body_close = _matching(text, body_open, "{", "}")
        if body_close < 0:
            continue
        body = text[body_open : body_close + 1]
        if not re.search(rf"\bregister\s*\(\s*{re.escape(variable)}\s*,", body):
            continue
        array = text[array_open : array_close + 1]
        tokens.extend(match.group(1) for match in TABLE_ENTRY.finditer(array))
    return tokens


def _method_count(text: str) -> int:
    """Heuristic: ``fn`` items inside each ``#[pymethods]`` block."""
    total = 0
    for match in PYMETHODS.finditer(text):
        brace = text.find("{", match.end())
        if brace < 0:
            continue
        depth = 0
        for index in range(brace, len(text)):
            char = text[index]
            if char == "{":
                depth += 1
            elif char == "}":
                depth -= 1
                if depth == 0:
                    total += len(METHOD_FN.findall(text[brace : index + 1]))
                    break
    return total


def _class_key(python_name: str) -> str:
    """``Part.Geom2d.BSplineCurve`` → ``Part.Geom2d::BSplineCurve`` (our key form)."""
    module, _, name = python_name.rpartition(".")
    return f"{module}::{name}" if module else python_name


def _our_methods(texts: dict[str, str]) -> dict[str, list[str]]:
    """Python-visible member names for every ``#[pyclass]`` in the bindings.

    A ``#[getter]``/``#[setter]`` pair becomes one property name, matching the
    attribute spelling in upstream's stubs. A pyclass that ``extends = PartShape``
    inherits that class's members, so `Part.Face.Surface` counts on `Part.Face`
    even though `Surface` is declared on the base.
    """
    classes: dict[str, str] = {}
    base_of: dict[str, str] = {}
    for text in texts.values():
        for match in PYCLASS_STRUCT.finditer(text):
            attrs, rust_type = match.groups()
            name = ATTR_NAME.search(attrs)
            module = ATTR_MODULE.search(attrs)
            python = name.group(1) if name else rust_type
            prefix = module.group(1) if module else "Part"
            key = f"{prefix}::{python}"
            classes[rust_type] = key
            base = ATTR_EXTENDS.search(attrs)
            if base is not None:
                base_of[key] = base.group(1)

    bases = {key: classes.get(rust, key) for key, rust in base_of.items()}
    direct: dict[str, set[str]] = {}
    for text in texts.values():
        for match in PYMETHODS_IMPL.finditer(text):
            key = classes.get(match.group(1))
            if key is None:
                continue
            brace = text.find("{", match.end())
            if brace < 0:
                continue
            end = _matching(text, brace, "{", "}")
            if end < 0:
                continue
            body = text[brace : end + 1]
            names = set(RUST_GETTER.findall(body)) | set(RUST_SETTER.findall(body))
            names.update(
                name
                for name in RUST_FN.findall(body)
                if not name.startswith("set_")
            )
            direct.setdefault(key, set()).update(names)

    def effective(key: str, seen: frozenset[str]) -> set[str]:
        members = set(direct.get(key, ()))
        base = bases.get(key)
        if base is not None and base not in seen:
            members |= effective(base, seen | {key})
        return members

    return {
        key: sorted(effective(key, frozenset()))
        for key in sorted(set(classes.values()) | set(direct))
    }


def derive_ours() -> dict:
    """Inventory the Python bindings and the object types of both Part crates."""
    binding_files = sorted(OURS_BINDING_DIR.glob("*.rs"))
    if not binding_files:
        raise SystemExit(f"part api audit: no Rust sources under {OURS_BINDING_DIR}")
    object_files = sorted(
        {path for source in OURS_OBJECT_SOURCES for path in (REPO / source).rglob("*.rs")}
    )

    binding_texts = {
        str(path.relative_to(REPO)): _read(path) for path in binding_files
    }
    object_texts = {str(path.relative_to(REPO)): _read(path) for path in object_files}
    python_names = _pyfunctions(binding_files)

    receivers: dict[str, str] = {}
    for text in binding_texts.values():
        receivers.update(_receiver_modules(text))

    registered_functions: set[str] = set()
    for text in binding_texts.values():
        for match in ADD_FUNCTION.finditer(text):
            receiver, rust_name = match.groups()
            module = receivers.get(receiver, "Part")
            registered_functions.add(f"{module}::{python_names.get(rust_name, rust_name)}")

    defined = set(python_names.values())
    registered_python_names = {name.split("::", 1)[1] for name in registered_functions}

    classes: set[str] = set()
    for text in binding_texts.values():
        for match in PYCLASS.finditer(text):
            attrs = match.group(1)
            name = ATTR_NAME.search(attrs)
            if name is None:
                continue
            module = ATTR_MODULE.search(attrs)
            prefix = module.group(1) if module else "Part"
            classes.add(f"{prefix}::{name.group(1)}")

    const_values: dict[str, str] = {}
    for text in object_texts.values():
        for match in TYPE_CONST.finditer(text):
            const_values[match.group(1)] = match.group(2)

    registered_types: set[str] = set()
    registered_constants: dict[str, str] = {}
    literal_registrations: set[str] = set()
    unresolved: set[str] = set()
    for text in object_texts.values():
        for token in _registration_tokens(text):
            if token.startswith('"'):
                value = token.strip('"')
                registered_types.add(value)
                literal_registrations.add(value)
            elif token in const_values:
                registered_types.add(const_values[token])
                registered_constants[token] = const_values[token]
            else:
                unresolved.add(token)

    markers: list[dict] = []
    for name, text in binding_texts.items():
        for lineno, line in enumerate(text.splitlines(), 1):
            for kind, needle in GAP_MARKERS:
                if needle in line:
                    markers.append(
                        {
                            "kind": kind,
                            "file": name,
                            "line": lineno,
                            "text": line.strip(),
                        }
                    )
    markers.sort(key=lambda entry: (entry["kind"], entry["file"], entry["line"]))

    by_kind: Counter = Counter(entry["kind"] for entry in markers)
    class_modules: Counter = Counter(entry.split("::")[0] for entry in classes)
    function_modules: Counter = Counter(entry.split("::")[0] for entry in registered_functions)

    methods = _our_methods(binding_texts)
    return {
        "source": OURS_SOURCE,
        "files": len(binding_files),
        "functions": {
            "registered": len(registered_functions),
            "defined": len(defined),
            "names": sorted(registered_functions),
            "defined_not_registered": sorted(defined - registered_python_names),
            "by_module": {key: function_modules[key] for key in sorted(function_modules)},
        },
        "classes": {
            "count": len(classes),
            "names": sorted(classes),
            "by_module": {key: class_modules[key] for key in sorted(class_modules)},
        },
        "object_types": {
            "sources": list(OURS_OBJECT_SOURCES),
            "files": len(object_files),
            "constants": len(registered_constants),
            "registered": len(registered_types),
            "names": sorted(registered_types),
            "constant_values": {
                key: registered_constants[key] for key in sorted(registered_constants)
            },
            "literal_registrations": sorted(literal_registrations),
            "unresolved": sorted(unresolved),
        },
        "methods_heuristic": sum(_method_count(text) for text in binding_texts.values()),
        "methods": {
            "count": sum(len(members) for members in methods.values()),
            "distinct": len(
                {member for members in methods.values() for member in members}
            ),
            "classes": methods,
        },
        "gap_markers": {
            "count": len(markers),
            "by_kind": {kind: by_kind.get(kind, 0) for kind, _ in GAP_MARKERS},
            "entries": markers,
        },
    }


# ---------------------------------------------------------------------------
# Upstream surface
# ---------------------------------------------------------------------------
def _upstream_functions(text: str) -> tuple[list[str], Counter, Counter]:
    """Qualified module functions and per-module/per-kind counters."""
    starts = [(match.start(), match.group(1)) for match in UPSTREAM_CLASS.finditer(text)]
    starts.append((len(text), None))

    names: list[str] = []
    by_module: Counter = Counter()
    by_kind: Counter = Counter()
    for (start, _cls), (end, _next) in zip(starts, starts[1:]):
        block = text[start:end]
        declared = UPSTREAM_MODULE.search(block)
        module = declared.group(1) if declared else "Part"
        qualified = "Part" if module == "Part" else f"Part.{module}"
        for match in UPSTREAM_METHOD.finditer(block):
            kind, name = match.groups()
            names.append(f"{qualified}::{name}")
            by_module[qualified] += 1
            by_kind[kind] += 1
    return sorted(set(names)), by_module, by_kind


def _upstream_classes(text: str) -> list[str]:
    """Python class names from ``addType(..., <module>, "Name")``."""
    modules = {"partModule": "Part"}
    for match in GET_ATTR.finditer(text):
        var, base, attr = match.groups()
        if base in ("module", "partModule"):
            modules[var] = f"Part.{attr}"
        else:
            modules[var] = f"{modules.get(base, 'Part')}.{attr}"

    names: set[str] = set()
    for match in ADD_TYPE.finditer(text):
        _cpp, var, name = match.groups()
        names.add(f"{modules.get(var, 'Part')}::{name}")
    return sorted(names)


def _strip_docstrings(text: str) -> str:
    """Remove ``\"\"\"…\"\"\"`` blocks (the stubs put author/licence notes in them)."""
    return re.sub(r'""".*?"""', "", text, flags=re.S)


def _pyi_methods(app: Path) -> dict[str, list[str]]:
    """Python-visible members declared by upstream's ``.pyi`` stubs.

    The stub's ``@export(PythonName="Part.X", …)`` is the authoritative class
    name (``TopoShape.pyi`` is ``Part.Shape``). Module stubs and the property
    contracts declare no class and are skipped. Dunders are left out because pyo3
    generates most of them and upstream does not list them uniformly.
    """
    methods: dict[str, set[str]] = {}
    for path in sorted(app.glob("*.pyi")):
        if path.stem.endswith(".module") or path.stem == "PropertyPythonContracts":
            continue
        text = _read(path)
        for export in PYI_EXPORT_CLASS.finditer(text):
            named = PYI_PYTHON_NAME.search(export.group(1))
            python_name = named.group(1) if named else f"Part.{export.group(2)}"
            body = _strip_docstrings(text[export.end() :])
            names = set(PYI_METHOD.findall(body)) | set(PYI_ATTRIBUTE.findall(body))
            names = {name for name in names if not name.startswith("__")}
            if names:
                methods.setdefault(_class_key(python_name), set()).update(names)
    return {key: sorted(values) for key, values in sorted(methods.items())}


def derive_upstream(app: Path) -> dict:
    """Inventory upstream's ``src/Mod/Part/App``."""
    app_cpp = _read(app / "AppPart.cpp")
    app_py_cpp = _read(app / "AppPartPy.cpp")

    functions, function_modules, function_kinds = _upstream_functions(app_py_cpp)
    classes = _upstream_classes(app_cpp)

    document_types: set[str] = set()
    typesystem_types: set[str] = set()
    macros: set[str] = set()
    for path in sorted(list(app.rglob("*.cpp")) + list(app.rglob("*.h"))):
        text = _read(path)
        document_types.update(
            match.group(1)
            for match in PROPERTY_SOURCE.finditer(text)
            if match.group(1).startswith("Part::")
        )
        typesystem_types.update(
            match.group(1)
            for match in TYPESYSTEM_SOURCE.finditer(text)
            if match.group(1).startswith("Part::")
        )
        macros.update(PART_TYPE_MACRO.findall(text))

    pyi = sorted(
        path.stem
        for path in app.glob("*.pyi")
        if not path.stem.endswith(".module") and path.stem != "PropertyPythonContracts"
    )
    pyimp = sorted(path.name[: -len("PyImp.cpp")] for path in app.glob("*PyImp.cpp"))
    stub_names = sorted(set(pyi) | set(pyimp))
    methods = _pyi_methods(app)

    class_modules: Counter = Counter(name.split("::")[0] for name in classes)

    return {
        "url": UPSTREAM_URL,
        "source": UPSTREAM_SOURCE,
        "revision": _revision(app),
        "files": {
            "cpp": sum(1 for _ in app.rglob("*.cpp")),
            "headers": sum(1 for _ in app.rglob("*.h")),
            "pyi": len(list(app.glob("*.pyi"))),
            "pyimp": len(list(app.glob("*PyImp.cpp"))),
        },
        "functions": {
            "count": len(functions),
            "names": functions,
            "by_module": {key: function_modules[key] for key in sorted(function_modules)},
            "by_kind": {key: function_kinds[key] for key in sorted(function_kinds)},
        },
        "classes": {
            "count": len(classes),
            "names": classes,
            "by_module": {key: class_modules[key] for key in sorted(class_modules)},
        },
        "stub_classes": {
            "count": len(stub_names),
            "pyi": len(pyi),
            "pyimp": len(pyimp),
            "names": stub_names,
        },
        "methods": {
            "count": sum(len(members) for members in methods.values()),
            "distinct": len(
                {member for members in methods.values() for member in members}
            ),
            "classes": methods,
        },
        "object_types": {
            "count": len(document_types),
            "names": sorted(document_types),
            "source": "PROPERTY_SOURCE",
        },
        "typesystem_types": {
            "count": len(typesystem_types),
            "names": sorted(typesystem_types),
        },
        "part_type_macros": {
            "count": len(macros),
            "names": sorted(macros),
        },
    }


# ---------------------------------------------------------------------------
# The contract
# ---------------------------------------------------------------------------
def derive(app: Path) -> dict:
    """Build the full contract payload; no timestamps, every list sorted."""
    ours = derive_ours()
    upstream = derive_upstream(app)
    ours_functions = set(ours["functions"]["names"])
    upstream_functions = set(upstream["functions"]["names"])
    ours_classes = set(ours["classes"]["names"])
    upstream_classes = set(upstream["classes"]["names"])
    ours_types = set(ours["object_types"]["names"])
    upstream_types = set(upstream["object_types"]["names"])

    ours_methods = ours["methods"]["classes"]
    upstream_methods = upstream["methods"]["classes"]
    shared_method_classes = sorted(set(ours_methods) & set(upstream_methods))
    missing_methods = {
        key: sorted(set(upstream_methods[key]) - set(ours_methods[key]))
        for key in shared_method_classes
    }
    missing_methods = {key: value for key, value in missing_methods.items() if value}

    ours_names = {name for members in ours_methods.values() for name in members}
    upstream_names = {name for members in upstream_methods.values() for name in members}
    covered_names = upstream_names & ours_names

    return {
        "schema": 2,
        "audit": "part-api",
        "upstream": upstream,
        "ours": ours,
        "missing": {
            "functions": sorted(upstream_functions - ours_functions),
            "classes": sorted(upstream_classes - ours_classes),
            "object_types": sorted(upstream_types - ours_types),
            "methods": missing_methods,
            "method_classes": sorted(set(upstream_methods) - set(ours_methods)),
        },
        "extra": {
            "functions": sorted(ours_functions - upstream_functions),
            "classes": sorted(ours_classes - upstream_classes),
            "object_types": sorted(ours_types - upstream_types),
        },
        "coverage": {
            "methods": {
                "upstream_declared": sum(
                    len(members) for members in upstream_methods.values()
                ),
                "ours_declared": sum(len(members) for members in ours_methods.values()),
                "upstream_distinct": len(upstream_names),
                "covered_distinct": len(covered_names),
                "fraction": (
                    round(len(covered_names) / len(upstream_names), 4)
                    if upstream_names
                    else 0.0
                ),
                "missing_distinct": sorted(upstream_names - ours_names),
                "shared_classes": len(shared_method_classes),
                "classes": shared_method_classes,
            }
        },
    }


def load_pinned(path: Path) -> dict:
    if not path.is_file():
        print(
            f"part api audit: pinned contract not found: {path} "
            "(run with --write to create it)",
            file=sys.stderr,
        )
        raise SystemExit(2)
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        print(f"part api audit: {path} is not valid JSON: {error}", file=sys.stderr)
        raise SystemExit(2) from error


def _comparable(payload: dict) -> dict:
    """Drop volatile fields before a drift comparison.

    The upstream revision is recorded, not checked (the counts are the check).
    Gap-marker line numbers move when unrelated code is inserted, so they are
    recorded but not compared. The method count is explicitly heuristic.
    """
    data = json.loads(json.dumps(payload))
    data["upstream"].pop("revision", None)
    data["ours"].pop("methods_heuristic", None)
    entries = data["ours"]["gap_markers"]["entries"]
    data["ours"]["gap_markers"]["entries"] = [
        f'{entry["kind"]}|{entry["file"]}|{entry["text"]}' for entry in entries
    ]
    return data


def _short(items: list[str], limit: int = 8) -> str:
    shown = ", ".join(items[:limit])
    if len(items) > limit:
        shown += f", … (+{len(items) - limit} more)"
    return shown


def diff(pinned: dict, found: dict, path: str = "") -> list[str]:
    """A short, named diff of two ``_comparable`` payloads."""
    problems: list[str] = []
    if isinstance(pinned, dict) and isinstance(found, dict):
        for key in sorted(set(pinned) | set(found)):
            child = f"{path}.{key}" if path else key
            if key not in pinned:
                problems.append(f"{child}: added in observed")
            elif key not in found:
                problems.append(f"{child}: missing from observed")
            else:
                problems.extend(diff(pinned[key], found[key], child))
        return problems

    if isinstance(pinned, list) and isinstance(found, list):
        if all(isinstance(item, str) for item in pinned + found):
            added = sorted(set(found) - set(pinned))
            removed = sorted(set(pinned) - set(found))
            if added:
                problems.append(f"{path}: +{_short(added)}")
            if removed:
                problems.append(f"{path}: -{_short(removed)}")
            return problems

        canonical = lambda item: json.dumps(item, sort_keys=True, ensure_ascii=False)
        added = sorted({canonical(item) for item in found} - {canonical(item) for item in pinned})
        removed = sorted({canonical(item) for item in pinned} - {canonical(item) for item in found})
        if added:
            problems.append(f"{path}: +{_short(added)}")
        if removed:
            problems.append(f"{path}: -{_short(removed)}")
        return problems

    if pinned != found:
        text = f"{path}: contract {pinned!r}, observed {found!r}"
        if isinstance(pinned, int) and isinstance(found, int):
            text += f" (delta {found - pinned:+d})"
        problems.append(text)
    return problems


def summary(found: dict) -> str:
    ours = found["ours"]
    upstream = found["upstream"]
    missing = found["missing"]
    revision = upstream.get("revision")
    tail = f" (upstream {revision})" if revision and revision != "unknown" else ""
    coverage = found.get("coverage", {}).get("methods", {})
    methods = (
        f"methods {coverage['covered_distinct']}/{coverage['upstream_distinct']} "
        f"upstream names ({coverage['fraction']:.1%}) in "
        f"{coverage['shared_classes']} classes; "
        if coverage
        else ""
    )
    return (
        "part api audit: "
        f"ours {ours['functions']['registered']} functions, "
        f"{ours['classes']['count']} classes, "
        f"{ours['object_types']['registered']} object types; "
        f"upstream {upstream['functions']['count']} functions, "
        f"{upstream['classes']['count']} classes, "
        f"{upstream['object_types']['count']} object types; "
        f"missing {len(missing['functions'])} functions, "
        f"{len(missing['classes'])} classes, "
        f"{len(missing['object_types'])} object types; "
        f"{methods}"
        f"{len(missing.get('methods', {}))} classes with method gaps; "
        f"{ours['gap_markers']['count']} gap markers{tail}"
    )


def _print_names(title: str, names: list[str]) -> None:
    print(f"  {title} ({len(names)}):")
    for name in names:
        print(f"    - {name}")


def _json_payload(
    root: Path, contract: Path, found: dict | None, drift: list[str] | None
) -> dict:
    if found is None:
        return {
            "root": str(root),
            "contract": str(contract),
            "skipped": True,
            "reason": "upstream checkout not found",
            "ok": True,
        }
    payload = {
        "root": str(root),
        "contract": str(contract),
        "skipped": False,
        "summary": summary(found),
        "ok": not drift,
    }
    if drift is not None:
        payload["drift"] = drift
    else:
        payload["derived"] = found
    return payload


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------
def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Derive the Part/OCCT API surface (ours and upstream's) and compare "
            "it with the pinned contract. CI runs --check only when a sibling "
            "freecad-upstream checkout exists."
        )
    )
    parser.add_argument(
        "--root",
        default=None,
        help=(
            "freecad-upstream checkout or its src/Mod/Part/App directory "
            f"(default: $FERROCAD_UPSTREAM or {DEFAULT_ROOT})"
        ),
    )
    parser.add_argument(
        "--contract",
        default=str(DEFAULT_CONTRACT),
        help=f"pinned JSON file (default: {DEFAULT_CONTRACT})",
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument(
        "--check", action="store_true", help="print a named delta and exit 1 on drift"
    )
    mode.add_argument("--write", action="store_true", help="re-generate the pinned contract")
    parser.add_argument("--json", action="store_true", help="print machine-readable output")
    args = parser.parse_args(argv)

    root = checkout_path(args.root)
    contract_path = Path(args.contract)

    if not root.exists():
        message = (
            f"part api audit: upstream checkout not found: {root} "
            "(set FERROCAD_UPSTREAM or --root)"
        )
        if args.write:
            print(message, file=sys.stderr)
            if args.json:
                print(json.dumps({"ok": False, "error": "checkout not found"}, indent=2))
            return 2
        print(f"{message}; skipping ({'no drift check' if args.check else 'nothing to do'})")
        if args.json:
            print(json.dumps(_json_payload(root, contract_path, None, None), indent=2))
        return 0

    app = upstream_app(root)
    if app is None:
        print(
            f"part api audit: {root} exists but has no Part/App sources "
            "(expected AppPart.cpp and AppPartPy.cpp); refusing to skip a "
            "partial checkout",
            file=sys.stderr,
        )
        if args.json:
            print(json.dumps({"ok": False, "error": "partial checkout"}, indent=2))
        return 2

    found = derive(app)

    if args.write:
        contract_path.parent.mkdir(parents=True, exist_ok=True)
        contract_path.write_text(
            json.dumps(found, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        if args.json:
            print(json.dumps(found, indent=2, ensure_ascii=False))
        else:
            print(f"wrote {contract_path}")
            print(summary(found))
        return 0

    if args.check:
        pinned = load_pinned(contract_path)
        problems = diff(_comparable(pinned), _comparable(found))
        if args.json:
            print(
                json.dumps(
                    _json_payload(root, contract_path, found, problems),
                    indent=2,
                    ensure_ascii=False,
                )
            )
            return 1 if problems else 0
        if problems:
            print(
                f"part api audit: {len(problems)} drift against {contract_path} "
                f"(declared upstream {pinned.get('upstream', {}).get('revision')}):"
            )
            for problem in problems:
                print(f"  {problem}")
            print(
                "Re-run `python3 tools/part_api_audit.py --write` after confirming "
                "the change."
            )
            return 1
        print(f"{summary(found)} — no drift against {contract_path}")
        revision = pinned.get("upstream", {}).get("revision")
        if revision and revision != found["upstream"]["revision"]:
            print(
                f"  note: revision differs: contract {revision}, "
                f"observed {found['upstream']['revision']} (surface still matches)"
            )
        return 0

    if args.json:
        print(json.dumps(found, indent=2, ensure_ascii=False))
        return 0

    print(summary(found))
    print("  ours:")
    print(f"    functions: {found['ours']['functions']['registered']} registered, "
          f"{found['ours']['functions']['defined']} defined")
    print(f"    classes: {found['ours']['classes']['count']}")
    print(f"    object types: {found['ours']['object_types']['registered']} registered, "
          f"{found['ours']['object_types']['constants']} constants")
    print(f"    gap markers: {found['ours']['gap_markers']['count']}")
    _print_names("missing functions", found["missing"]["functions"])
    _print_names("missing classes", found["missing"]["classes"])
    _print_names("missing object types", found["missing"]["object_types"])
    print(f"  source: {app}")
    print(
        f"  contract: {contract_path} "
        f"({'present' if contract_path.is_file() else 'not written yet'})"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
