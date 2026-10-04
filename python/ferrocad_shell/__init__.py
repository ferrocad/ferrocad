"""App-shell helpers used by the FerroCAD shell library.

The shell drives the document model **only** through these functions, so the
shell is a client of the same public ``FreeCAD`` API a workbench would use.
Each entry point returns a JSON string the Rust side deserialises.
"""

from __future__ import annotations

import contextlib
import io
import json
import traceback

import FreeCAD

# The namespace the console evaluates in; populated by :func:`bootstrap`.
_NAMESPACE: dict = {}

# Property type id -> editor kind. A type absent here is read-only in the pane.
_EDITABLE_KINDS = {
    "App::PropertyString": "text",
    "App::PropertyBool": "bool",
    "App::PropertyInteger": "int",
    "App::PropertyFloat": "float",
    "App::PropertyLength": "quantity",
    "App::PropertyDistance": "quantity",
    "App::PropertyAngle": "quantity",
    "App::PropertyQuantity": "quantity",
    "App::PropertyEnumeration": "enum",
}


def _json(payload) -> str:
    return json.dumps(payload)


def hello() -> str:
    """A first console line, proving the interpreter is live."""
    version = ".".join(FreeCAD.Version()[:3])
    return f"hello from Python {version} — FreeCAD backend: {FreeCAD.backend}"


def load_workbenches(dirs_json: str = "[]") -> str:
    """Best-effort workbench loading.

    For every ``<dir>/<Workbench>/Init.py`` and ``InitGui.py`` the script is
    executed. A failure is **recorded, not raised**: one broken workbench must not
    take down the app (FreeCAD isolates them the same way). Returns JSON with the
    scripts that loaded and the tracebacks that did not.
    """
    import os

    try:
        dirs = json.loads(dirs_json)
    except Exception:
        dirs = []

    loaded = []
    errors = []
    for root in dirs:
        if not os.path.isdir(root):
            continue
        for name in sorted(os.listdir(root)):
            workbench_dir = os.path.join(root, name)
            if not os.path.isdir(workbench_dir):
                continue
            for script in ("Init.py", "InitGui.py"):
                path = os.path.join(workbench_dir, script)
                if not os.path.isfile(path):
                    continue
                namespace = {"__file__": path, "__name__": f"{name}.{script[:-3]}"}
                try:
                    with open(path, encoding="utf-8") as handle:
                        source = handle.read()
                    exec(compile(source, path, "exec"), namespace)
                    loaded.append(f"{name}/{script}")
                except Exception:
                    errors.append(
                        {
                            "workbench": name,
                            "script": script,
                            "traceback": traceback.format_exc(),
                        }
                    )
    return _json({"loaded": loaded, "errors": errors})


def bootstrap(doc_name: str = "Shell") -> str:
    """Create (or recreate) the sample document and report host status."""
    if FreeCAD.getDocument(doc_name) is not None:
        FreeCAD.closeDocument(doc_name)
    doc = FreeCAD.newDocument(doc_name)

    # A small parametric document: two feature objects wired by an expression,
    # plus a group so the inspector shows some structure.
    params = doc.addObject("App::FeaturePython", "Params")
    params.addProperty("App::PropertyLength", "Length", "Base", "Source length")
    params.addProperty("App::PropertyEnumeration", "Mode", "Base", "Operation mode")
    params.Mode = ["Fast", "Accurate"]
    params.Length = FreeCAD.Units.Quantity("10 mm")

    derived = doc.addObject("App::FeaturePython", "Derived")
    derived.addProperty("App::PropertyLength", "Result", "Base", "Computed result")
    derived.setExpression("Result", "Params.Length * 2")

    group = doc.addObject("App::DocumentObjectGroup", "Group")
    group.addObject(params)

    doc.recompute()

    _NAMESPACE.clear()
    _NAMESPACE.update({"FreeCAD": FreeCAD, "App": FreeCAD, "doc": doc})

    return _json(
        {
            "version": ".".join(FreeCAD.Version()[:3]),
            "backend": FreeCAD.backend,
            "document": doc.Name,
        }
    )


def _active_doc():
    doc = FreeCAD.ActiveDocument
    if doc is None:
        raise RuntimeError("no active document")
    return doc


def model_tree() -> str:
    """Documents and their objects, for the inspector pane."""
    docs = []
    for name in FreeCAD.listDocuments():
        doc = FreeCAD.getDocument(name)
        if doc is None:
            continue
        objects = [
            {"name": obj.Name, "label": obj.Label, "type": obj.TypeId}
            for obj in doc.Objects
        ]
        docs.append({"name": doc.Name, "label": doc.Label, "objects": objects})
    return _json(docs)


def _expression_properties(obj) -> set:
    """Names of properties that are driven by an expression (not freely settable)."""
    try:
        return {name for name, _ in obj.ExpressionEngine}
    except Exception:
        return set()


def properties(object_name: str) -> str:
    """Property rows for one object, for the property editor pane.

    ``editable`` marks a property the editor can change, and ``kind`` selects the
    control (and the coercion in :func:`set_property`). A property set by an
    expression is read-only, because a recompute would overwrite the edit.
    """
    doc = _active_doc()
    obj = doc.getObject(object_name)
    if obj is None:
        return _json([])
    driven = _expression_properties(obj)
    rows = []
    for prop in obj.PropertiesList:
        try:
            value = getattr(obj, prop)
        except Exception:
            value = "<error>"
        try:
            status = ", ".join(obj.getTypeOfProperty(prop))
        except Exception:
            status = ""
        type_id = obj.getTypeIdOfProperty(prop)
        kind = _EDITABLE_KINDS.get(type_id, "")
        rows.append(
            {
                "name": prop,
                "type": type_id,
                "value": str(value),
                "status": status,
                "editable": bool(kind) and prop not in driven,
                "kind": kind,
            }
        )
    return _json(rows)


def _coerce(kind: str, text: str):
    """Turn an editor string into the Python value a property setter expects."""
    if kind in ("text", "enum"):
        return text
    if kind == "bool":
        return text.strip().lower() in ("1", "true", "yes", "on")
    if kind == "int":
        return int(float(text))
    if kind == "float":
        return float(text)
    if kind == "quantity":
        return FreeCAD.Units.Quantity(text)
    raise ValueError(f"unsupported editor kind {kind!r}")


def set_property(object_name: str, prop: str, text: str) -> str:
    """Set one property from its editor string, in a named transaction.

    Returns ``{"ok": bool, "error": str}``. The transaction makes the edit a
    single undo step; recompute then propagates it (for example to an
    expression-driven property elsewhere in the document).
    """
    doc = _active_doc()
    obj = doc.getObject(object_name)
    if obj is None:
        return _json({"ok": False, "error": f"no object {object_name!r}"})
    if prop not in obj.PropertiesList:
        return _json({"ok": False, "error": f"no property {prop!r}"})
    kind = _EDITABLE_KINDS.get(obj.getTypeIdOfProperty(prop), "")
    if not kind:
        return _json({"ok": False, "error": f"{prop} is not editable"})
    if prop in _expression_properties(obj):
        return _json({"ok": False, "error": f"{prop} is set by an expression"})
    try:
        value = _coerce(kind, text)
    except Exception as exc:
        return _json({"ok": False, "error": str(exc)})

    doc.openTransaction(f"Edit {prop}")
    try:
        setattr(obj, prop, value)
    except Exception as exc:
        doc.abortTransaction()
        return _json({"ok": False, "error": str(exc)})
    doc.commitTransaction()
    doc.recompute()
    return _json({"ok": True, "error": ""})


def evaluate(code: str) -> str:
    """Evaluate one console line, capturing stdout/stderr and any traceback."""
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf), contextlib.redirect_stderr(buf):
        try:
            try:
                value = eval(code, _NAMESPACE)
                if value is not None:
                    print(repr(value))
            except SyntaxError:
                exec(code, _NAMESPACE)
            return _json({"ok": True, "output": buf.getvalue().rstrip("\n")})
        except Exception:
            buf.write(traceback.format_exc())
            return _json({"ok": False, "output": buf.getvalue().rstrip("\n")})
