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
import FreeCADGui

# The namespace the console evaluates in; populated by :func:`bootstrap`.
_NAMESPACE: dict = {}

# Beyond the model, the shell asks for the pieces the UI needs: the tessellated
# shapes to draw (:func:`stage_meshes`), the document edit a manipulator commit
# produces (:func:`interaction`), the workbench actions (:func:`commands`,
# :func:`run_command`), and the selection snapshot (:func:`set_selection`). All go
# through the module hooks or ``FreeCADGui``, so this script names no workbench.

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

    # The sample document is deliberately geometry-free: a shape needs a workbench's
    # display hook (see `FreeCAD.register_display_provider`), which the edition's app
    # script registers. The base app keeps its `pivy.coin` sample scene.
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


def add_namespace(name: str, value) -> None:
    """Expose ``value`` to the console under ``name``.

    Workbenches call this from their app script after ``bootstrap`` so their commands
    are reachable at the prompt (``Toy.makeSlab(...)``) without an import. The
    namespace survives console submissions; it is rebuilt only by ``bootstrap``.
    """
    _NAMESPACE[name] = value


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


def _base_link(obj):
    """The ``(object_name, [subnames])`` a `Base` link-sub property holds, if any."""
    try:
        link = obj.Base
    except Exception:
        return None
    if not isinstance(link, tuple) or not link:
        return None
    linked = link[0]
    name = getattr(linked, "Name", None)
    if name is None:
        return None
    return name, list(link[1]) if len(link) > 1 else []


def stage_meshes() -> str:
    """The tessellated shapes the viewport draws, as JSON.

    Only objects a registered module can display are included (the mesh comes from
    ``FreeCAD.display_mesh``, so this module names no workbench), and an object
    consumed by a feature (its ``Base``) is skipped: the feature's shape replaces it.
    ``entity`` is the object's 1-based position in the document, so the host can map a
    pick back to a name.

    Returns ``[]`` when no module is loaded, leaving the shell's `pivy.coin` sample
    scene in place.
    """
    try:
        doc = _active_doc()
    except Exception:
        return "[]"

    objects = list(doc.Objects)
    consumed = set()
    for obj in objects:
        link = _base_link(obj)
        if link is not None:
            consumed.add(link[0])

    meshes = []
    for index, obj in enumerate(objects, start=1):
        if obj.Name in consumed:
            continue
        mesh = FreeCAD.display_mesh(obj)
        if mesh is None:
            continue
        meshes.append({"object": obj.Name, "label": obj.Label, "entity": index, **mesh})
    return _json(meshes)


def interaction(
    object_name: str, manipulator: str, action: str, element: str, values_json: str
) -> str:
    """Apply a manipulator commit through the module that supports it.

    The manipulator is presentation-side (`ferrocad_stage::Manipulator`); the edit it
    implies belongs to the workbench (``FreeCAD.register_interaction_provider``). This
    function only resolves the object and marshals the request; the shell names no
    workbench and links no kernel. Returns ``{"ok": ...}``.
    """
    try:
        doc = _active_doc()
    except Exception as exc:
        return _json({"ok": False, "error": str(exc)})
    base = doc.getObject(object_name)
    if base is None:
        return _json({"ok": False, "error": f"no object named {object_name}"})
    try:
        values = [float(value) for value in json.loads(values_json)]
    except Exception as exc:
        return _json({"ok": False, "error": f"bad values: {exc}"})

    result = FreeCAD.interaction(base, manipulator, action, element, values)
    if result is None:
        return _json(
            {"ok": False, "error": f"no module handles {manipulator} {action}"}
        )
    return _json(result)


def commands() -> str:
    """The registered workbench commands, with resources and live state, as JSON.

    A command is `FreeCADGui`-registered by the workbench (see
    ``docs/actions-and-tools.md``); this script only marshals the list, so the
    shell names no workbench either.
    """
    return _json(FreeCADGui._command_list())


def run_command(name: str) -> str:
    """Invoke a workbench command by id; returns ``{ok, status?, error?, ...}``."""
    return _json(FreeCADGui._run(name))


def set_selection(pairs_json: str) -> str:
    """Replace ``FreeCADGui.Selection`` with the shell's selection.

    The payload is ``[{"object": name, "elements": [...]}, ...]``; the facade
    snapshot takes ``(name, elements)`` pairs, so the two are translated here. The
    shell owns the selection; this push is what makes it visible to a command's
    ``IsActive`` and ``Activated`` (see ``docs/actions-and-tools.md`` §5).
    """
    items = json.loads(pairs_json)
    FreeCADGui.Selection._set(
        [(item["object"], item.get("elements") or []) for item in items]
    )
    return _json({"ok": True})


# ---------------------------------------------------------------------------
# Built-in commands (A2)
#
# These are app-level, not workbench-level: they act on any document object and
# every edition gets them because every edition imports this module. They follow
# the same contract as a workbench command, and each owns its transaction.
# ---------------------------------------------------------------------------


def _selection() -> list:
    """The selected document objects (empty when nothing resolves)."""
    return list(FreeCADGui.Selection.getSelection())


class _DeleteCommand:
    """``App_Delete`` — remove the selected objects as one undo step."""

    def GetResources(self):
        return {
            "MenuText": "Delete",
            "ToolTip": "Delete the selected objects",
            "Accel": "Del",
            "Group": "Edit",
            "CmdType": "ForEdit",
        }

    def IsActive(self):
        return FreeCAD.ActiveDocument is not None and bool(_selection())

    def Activated(self):
        doc = FreeCAD.ActiveDocument
        objects = _selection()
        if not objects:
            return {"status": "nothing selected"}
        doc.openTransaction("Delete")
        try:
            for obj in objects:
                doc.removeObject(obj.Name)
            doc.recompute()
            doc.commitTransaction()
        except Exception as exc:  # noqa: BLE001 - one transaction, then report
            doc.abortTransaction()
            return {"status": f"delete failed: {exc}"}
        return {"status": f"deleted {len(objects)} object(s)"}


class _MoveCommand:
    """``App_Move`` — translate the selection by ``(dx, dy, dz)``.

    The non-interactive twin of the gizmo. A3 replaces the pending-dialog branch
    with the shell's non-blocking dialog; until then the command is usable from
    Python: ``FreeCADGui.runCommand("App_Move", 10, 0, 0)``.
    """

    def GetResources(self):
        return {
            "MenuText": "Move",
            "ToolTip": "Move the selected objects by a delta",
            "Accel": "M",
            "Group": "Edit",
            "CmdType": "ForEdit",
        }

    def IsActive(self):
        return FreeCAD.ActiveDocument is not None and bool(_selection())

    def Activated(self, dx=None, dy=None, dz=None):
        if dx is None and dy is None and dz is None:
            return {"status": "Move needs a delta (the dialog arrives with A3)"}
        doc = FreeCAD.ActiveDocument
        objects = _selection()
        if not objects:
            return {"status": "nothing selected"}
        delta = FreeCAD.Placement(
            FreeCAD.Vector(float(dx or 0.0), float(dy or 0.0), float(dz or 0.0))
        )
        doc.openTransaction("Move")
        try:
            for obj in objects:
                obj.Placement = delta * obj.Placement
            doc.recompute()
            doc.commitTransaction()
        except Exception as exc:  # noqa: BLE001
            doc.abortTransaction()
            return {"status": f"move failed: {exc}"}
        return {"status": f"moved {len(objects)} object(s)"}


FreeCADGui.addCommand("App_Delete", _DeleteCommand())
FreeCADGui.addCommand("App_Move", _MoveCommand())
