"""App-shell helpers used by the FerroCAD shell library.

The shell drives the document model **only** through these functions, so the
shell is a client of the same public ``FreeCAD`` API a workbench would use.
Each entry point returns a JSON string the Rust side deserialises.
"""

from __future__ import annotations

import contextlib
import io
import json
import sys
import traceback

import FreeCAD
import FreeCADGui
import FerroCAD
import FerroCADGui

# The namespace the console evaluates in; populated by :func:`bootstrap`.
_NAMESPACE: dict = {}

# Beyond the model, the shell asks for the pieces the UI needs: the tessellated
# shapes to draw (:func:`stage_meshes`), the document edit a manipulator commit
# produces (:func:`interaction`), the workbench actions (:func:`commands`,
# :func:`run_command`), and the selection snapshot (:func:`set_selection`). All go
# through the FerroCAD module hooks or ``FreeCADGui``/``FerroCADGui``, so this
# script names no workbench.

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

# The synthetic settings root. It is not a document: ``/`` is illegal in a
# FreeCAD object name, so ``Settings/<Feature>`` cannot collide with one.
_SETTINGS_ROOT = "Settings"
_SETTINGS_TYPE = "FerroCAD::Settings"

# Presentation-wide preferences live under the view group, next to the other
# settings the shell owns (a GPU adapter belongs to the shell, not a mod).
_VIEW_PREFERENCES = "User parameter:BaseApp/Preferences/View"
_GPU_ADAPTER_SETTING = "GpuAdapter"
_GPU_ADAPTER_FORMS = '"auto", "backend:vulkan|gl|metal|dx12", or "adapter:<name substring>"'
_GPU_BACKENDS = ("vulkan", "gl", "metal", "dx12")


def _json(payload) -> str:
    return json.dumps(payload)


def hello() -> str:
    """A first console line, proving the interpreter is live."""
    version = ".".join(FreeCAD.Version()[:3])
    return f"hello from Python {version} — FreeCAD backend: {FreeCAD.backend}"


def gpu_adapter() -> str:
    """The persisted viewport GPU adapter setting (``auto`` by default).

    The shell reads this when it creates the viewport, never per frame, and
    parses it with ``ferrocad_wgpu::AdapterChoice``. It is a plain string rather
    than JSON: the bridge extracts this one value directly. The value lives in
    ``User parameter:BaseApp/Preferences/View`` so it survives a restart through
    the same file-backed parameter store as the other view settings.
    """
    return FreeCAD.ParamGet(_VIEW_PREFERENCES).GetString(_GPU_ADAPTER_SETTING, "auto")


def _gpu_adapter_ok(value: str) -> bool:
    """Whether ``value`` has the shape the Rust parser accepts.

    This is deliberately the same light grammar the parser enforces (a prefix
    and a non-empty remainder), so the panel can report an obvious mistake
    instead of persisting a value that would only fall back to ``auto`` on the
    next boot. The Rust side remains the authority for the final choice.
    """
    lowered = value.lower()
    if lowered == "auto":
        return True
    if lowered.startswith("backend:"):
        return lowered[len("backend:"):].strip() in _GPU_BACKENDS
    if lowered.startswith("adapter:"):
        return bool(value[len("adapter:"):].strip())
    return False


def _gpu_adapter_error(value: str) -> str:
    return f"invalid GPU adapter setting {value!r}; use {_GPU_ADAPTER_FORMS}"


def set_gpu_adapter(payload_json: str) -> str:
    """Persist the viewport's GPU adapter choice from the Dev Tools panel.

    The payload is ``{"value": "adapter:Llvm"}``. The accepted grammar is
    ``auto``, ``backend:vulkan|gl|metal|dx12``, or ``adapter:<name substring>``;
    the value is trimmed and stored. A malformed payload, an empty value or an
    unrecognised form is **reported, never raised**: the shell calls this across
    the interpreter boundary and shows the error in the panel's status line.
    Returns ``{"ok": bool, "value": str, "error": str}``.
    """
    try:
        payload = json.loads(payload_json)
        value = payload.get("value")
    except Exception as exc:  # noqa: BLE001 - a bad payload is a result, not a crash
        return _json({"ok": False, "value": "", "error": str(exc)})

    if not isinstance(value, str):
        return _json(
            {
                "ok": False,
                "value": "",
                "error": f"GPU adapter setting must be a string; use {_GPU_ADAPTER_FORMS}",
            }
        )

    value = value.strip()
    if not value or not _gpu_adapter_ok(value):
        return _json({"ok": False, "value": "", "error": _gpu_adapter_error(value)})

    FreeCAD.ParamGet(_VIEW_PREFERENCES).SetString(_GPU_ADAPTER_SETTING, value)
    return _json({"ok": True, "value": value, "error": ""})


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
    # A dialog token belongs to the document that requested it: opening a fresh
    # document drops any callback still waiting for the old one.
    FreeCADGui._cancel_all()
    # ``FerroCAD.closeDocument``/``newDocument`` are the lifecycle entry points:
    # they fire ``FerroCADGui``'s document-observer slot (deactivated before the
    # closed document disappears, then closed; created/activated for the new
    # one), so a provider detaches its per-document effects immediately. The
    # upstream ``FreeCAD`` functions keep their exact semantics and do not.
    if FreeCAD.getDocument(doc_name) is not None:
        FerroCAD.closeDocument(doc_name)
    doc = FerroCAD.newDocument(doc_name)

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
    # display hook (see `FerroCAD.register_display_provider`), which the edition's app
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
    """Documents and their objects, for the inspector pane.

    A synthetic ``Settings`` root comes first when at least one provider feature
    is registered. It is **not** a document: it holds one
    ``Settings/<Feature>`` node per feature, whose rows are the active
    provider's schema (see :func:`properties`). With nothing registered there is
    nothing to configure, so the list is exactly the documents.
    """
    docs = []
    settings = _settings_root()
    if settings["objects"]:
        docs.append(settings)
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


def _settings_root() -> dict:
    """The synthetic settings root: one node per registered feature."""
    features = []
    seen = set()
    for manifest in FerroCADGui.providers():
        feature = manifest["feature"]
        if feature in seen:
            continue
        seen.add(feature)
        features.append(feature)
    objects = [
        {
            "name": f"{_SETTINGS_ROOT}/{feature}",
            "label": str(feature).title(),
            "type": _SETTINGS_TYPE,
        }
        for feature in features
    ]
    return {"name": _SETTINGS_ROOT, "label": _SETTINGS_ROOT, "objects": objects}


def _settings_feature(object_name):
    """The registered feature id for a synthetic ``Settings/<Feature>`` path.

    Returns ``None`` when the path is not in the synthetic namespace. A segment
    that does not name a registered feature is returned as-is, so the caller can
    report the unknown path in the usual style (``[]`` / ``ok: false``).
    """
    prefix = _SETTINGS_ROOT + "/"
    if not isinstance(object_name, str) or not object_name.startswith(prefix):
        return None
    node = object_name[len(prefix):]
    features = []
    for manifest in FerroCADGui.providers():
        feature = manifest["feature"]
        if feature not in features:
            features.append(feature)
    if node in features:
        return node
    for feature in features:
        if str(feature).title() == node:
            return feature
    return node


def _settings_properties(feature: str) -> list:
    """The rows under ``Settings/<Feature>``: the provider selector, then settings."""
    manifests = FerroCADGui.providers(feature)
    if not manifests:
        return []
    active = FerroCADGui.activeProvider(feature)
    label = next(
        (item["label"] for item in manifests if item["backend"] == active),
        str(active or ""),
    )
    rows = [
        {
            "name": "provider",
            "label": "Provider",
            "type": "App::PropertyEnumeration",
            "value": label,
            "status": "provider",
            "editable": True,
            "kind": "enum",
            "choices": [item["label"] for item in manifests],
        }
    ]
    rows.extend(FerroCADGui.settings(feature))
    return rows


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

    ``Settings/<Feature>`` is resolved first and needs no active document: its
    rows are the feature's provider selector followed by the active provider's
    settings schema. An unknown ``Settings/...`` path is ``[]``. Document names
    never contain ``/``, so the two namespaces cannot collide.
    """
    feature = _settings_feature(object_name)
    if feature is not None:
        return _json(_settings_properties(feature))
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

    ``Settings/<Feature>`` routes here too: ``provider`` switches the active
    backend (label or backend id), anything else goes to
    :func:`FerroCADGui.setSetting`. Neither needs an active document.
    """
    feature = _settings_feature(object_name)
    if feature is not None:
        return _json(_set_setting(feature, object_name, prop, text))
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


def _set_setting(feature: str, object_name: str, prop: str, text: str) -> dict:
    """Route one ``Settings/<Feature>`` edit; never raises into the shell."""
    manifests = FerroCADGui.providers(feature)
    if not manifests:
        return {"ok": False, "error": f"no object {object_name!r}"}
    if prop == "provider":
        backend = next(
            (
                item["backend"]
                for item in manifests
                if text in (item["label"], item["backend"])
            ),
            None,
        )
        if backend is None:
            return {
                "ok": False,
                "error": f"no provider {text!r} for feature {feature!r}",
            }
        try:
            return FerroCADGui.setActiveProvider(feature, backend)
        except Exception as exc:  # noqa: BLE001 - a bad id is not fatal here
            return {"ok": False, "error": str(exc)}
    return FerroCADGui.setSetting(feature, prop, text)


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
    ``FerroCAD.display_mesh``, so this module names no workbench), and an object
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
        mesh = FerroCAD.display_mesh(obj)
        if mesh is None:
            continue
        meshes.append({"object": obj.Name, "label": obj.Label, "entity": index, **mesh})
    return _json(meshes)


def interaction(
    object_name: str, manipulator: str, action: str, element: str, values_json: str
) -> str:
    """Apply a manipulator commit through the module that supports it.

    The manipulator is presentation-side (`ferrocad_stage::Manipulator`); the edit it
    implies belongs to the workbench (``FerroCAD.register_interaction_provider``). This
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

    result = FerroCAD.interaction(base, manipulator, action, element, values)
    if result is None:
        return _json(
            {"ok": False, "error": f"no module handles {manipulator} {action}"}
        )
    return _json(result)


def manipulator_specs(payload_json: str) -> str:
    """The handles the mods declare for the shell's current selection, as JSON.

    ``payload_json`` is ``{"object": "Slab", "element": "Face3"}``. A missing key
    means ``""`` for the object and ``None`` for the element. The answer is
    ``{"specs": [...]}`` with the declarations in provider order.

    The selection path must not break because of a mod: a provider that raises is
    reported and skipped by ``FerroCAD.manipulator_specs``, and a spec that does
    not serialise is reported here and dropped while its siblings survive.
    """
    try:
        payload = json.loads(payload_json)
    except Exception:
        payload = {}
    if not isinstance(payload, dict):
        payload = {}

    object_name = payload.get("object")
    if object_name is None:
        object_name = ""
    element = payload.get("element")

    try:
        specs = FerroCAD.manipulator_specs(object_name, element)
    except Exception as exc:  # noqa: BLE001 - selection must never fail here
        print(
            "ferrocad_shell: manipulator specs failed: %s" % exc,
            file=sys.stderr,
        )
        specs = []

    serialisable = []
    for spec in specs:
        try:
            json.dumps(spec)
        except (TypeError, ValueError) as exc:
            print(
                "ferrocad_shell: dropping a non-serialisable manipulator spec: %s"
                % exc,
                file=sys.stderr,
            )
            continue
        serialisable.append(spec)
    return _json({"specs": serialisable})


def commands() -> str:
    """The registered workbench commands, with resources and live state, as JSON.

    A command is `FreeCADGui`-registered by the workbench (see
    ``docs/workbenches/actions-and-tools.md``); this script only marshals the list, so the
    shell names no workbench either.

    FerroCAD extends ``GetResources()`` with two optional keys the upstream facade does
    not know about: ``FerroCAD_Surface`` (which chrome surface owns the command) and
    ``FerroCAD_Parent`` (the surface's primary command id, for a row in that control's
    popover). They are added here, in the shell bridge, so ``FreeCADGui`` itself stays
    upstream-shaped; the JSON keeps the neutral field names ``surface``/``parent``.
    """
    entries = FreeCADGui._command_list()
    registry = getattr(FreeCADGui, "_COMMANDS", {})
    read_resources = getattr(FreeCADGui, "_resources", None)
    for entry in entries:
        command = registry.get(entry["id"])
        resources = read_resources(command) if command is not None and read_resources else {}
        entry["surface"] = str(resources.get("FerroCAD_Surface") or "")
        entry["parent"] = str(resources.get("FerroCAD_Parent") or "")
    return _json(entries)


def run_command(name: str) -> str:
    """Invoke a workbench command by id; returns ``{ok, status?, dialog?, token?}``."""
    return _json(FreeCADGui._run(name))


def dialog_accept(payload_json: str) -> str:
    """Accept an open dialog: ``{"token": ..., "values": {...}}``.

    The shell has already validated and typed the values; this hands them to the
    command's ``on_accept`` (which owns its transaction) and returns its result.
    """
    payload = json.loads(payload_json)
    return _json(
        FreeCADGui._accept(payload["token"], payload.get("values") or {})
    )


def dialog_cancel(token: str) -> str:
    """Cancel an open dialog and drop its token."""
    return _json(FreeCADGui._cancel(token))


def set_selection(pairs_json: str) -> str:
    """Replace ``FreeCADGui.Selection`` with the shell's selection.

    The payload is ``[{"object": name, "elements": [...]}, ...]``; the facade
    snapshot takes ``(name, elements)`` pairs, so the two are translated here. The
    shell owns the selection; this push is what makes it visible to a command's
    ``IsActive`` and ``Activated`` (see ``docs/workbenches/actions-and-tools.md`` §5).
    """
    items = json.loads(pairs_json)
    FreeCADGui.Selection._set(
        [(item["object"], item.get("elements") or []) for item in items]
    )
    return _json({"ok": True})


def set_view_state(state_json: str) -> str:
    """Mirror the shell's presentation state to ``FerroCADGui.ViewState``.

    A checkable command reads it in ``IsChecked``; the shell pushes it before it
    re-lists the commands, so a ``{"view": …}`` request closes the loop.
    """
    state = json.loads(state_json)
    FerroCADGui.ViewState.grid = bool(state.get("grid", False))
    FerroCADGui.ViewState.axis_cross = bool(state.get("axis_cross", True))
    FerroCADGui.ViewState.projection = str(state.get("projection", "persp"))
    FerroCADGui.ViewState.grid_minor = float(state.get("grid_minor", 10.0))
    FerroCADGui.ViewState.grid_major = float(state.get("grid_major", 100.0))
    radius = state.get("grid_radius")
    FerroCADGui.ViewState.grid_radius = None if radius is None else float(radius)
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
            "FerroCAD_Surface": "dock",
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

    The non-interactive twin of the gizmo. With explicit arguments it applies them
    and returns a status; with none it returns a **dialog** the shell hosts, whose
    callback owns the transaction (A3).
    """

    def GetResources(self):
        return {
            "MenuText": "Move",
            "ToolTip": "Move the selected objects by a delta",
            "Accel": "Ctrl+Shift+M",
            "Group": "Edit",
            "CmdType": "ForEdit",
            "FerroCAD_Surface": "dock",
        }

    def IsActive(self):
        return FreeCAD.ActiveDocument is not None and bool(_selection())

    def Activated(self, dx=None, dy=None, dz=None):
        if dx is not None or dy is not None or dz is not None:
            return _move_selection(float(dx or 0.0), float(dy or 0.0), float(dz or 0.0))
        return {
            "title": "Move",
            "accept": "Move",
            "fields": [
                {"name": "dx", "label": "ΔX", "kind": "quantity", "value": 0.0},
                {"name": "dy", "label": "ΔY", "kind": "quantity", "value": 0.0},
                {"name": "dz", "label": "ΔZ", "kind": "quantity", "value": 0.0},
            ],
            "on_accept": lambda values: _move_selection(
                float(values.get("dx", 0.0)),
                float(values.get("dy", 0.0)),
                float(values.get("dz", 0.0)),
            ),
        }


def _move_selection(dx, dy, dz):
    """Translate the current selection as one transaction."""
    doc = FreeCAD.ActiveDocument
    objects = _selection()
    if not objects:
        return {"status": "nothing selected"}
    delta = FreeCAD.Placement(FreeCAD.Vector(dx, dy, dz))
    doc.openTransaction("Move")
    try:
        for obj in objects:
            obj.Placement = delta * obj.Placement
        doc.recompute()
        doc.commitTransaction()
    except Exception as exc:  # noqa: BLE001 - one transaction, then report
        doc.abortTransaction()
        return {"status": f"move failed: {exc}"}
    return {"status": f"moved {len(objects)} object(s)"}


FreeCADGui.addCommand("App_Delete", _DeleteCommand())
FreeCADGui.addCommand("App_Move", _MoveCommand())


#: The provider feature whose backend may own the drawn grid. The shell names
#: the feature it controls, never a backend: which provider draws (and how) is
#: the registry's answer.
_GRID_FEATURE = "grid"

#: The provider feature whose backend may own the drawn axis cross. Like the
#: grid, the shell names the feature it controls and the registry names the
#: backend that draws it.
_AXIS_FEATURE = "axis"

#: The one grid-settings surface the grid commands select (S4).
_GRID_SETTINGS_NODE = "Settings/Grid"

#: The one axis-settings surface the axis settings command selects.
_AXIS_SETTINGS_NODE = "Settings/Axis"


def grid_summary() -> str:
    """The grid's provider and spacing for the shell's popover.

    ``backend``/``label`` name the active provider and ``spacing`` is its
    ``minor`` row's display value (``"10 mm"``); ``providers`` lists every
    registered grid backend with its ``active`` flag. The shape is closed: when
    no grid provider is registered, the same keys are present with ``None`` and
    an empty list, so the popover never has to special-case the empty state.
    """
    manifests = FerroCADGui.providers(_GRID_FEATURE)
    providers = [
        {
            "backend": manifest["backend"],
            "label": manifest["label"],
            "active": bool(manifest["active"]),
        }
        for manifest in manifests
    ]
    if not manifests:
        return _json(
            {
                "feature": _GRID_FEATURE,
                "backend": None,
                "label": None,
                "spacing": None,
                "providers": [],
            }
        )

    active = next(
        (manifest for manifest in manifests if manifest["active"]), manifests[0]
    )
    rows = {row["name"]: row for row in FerroCADGui.settings(_GRID_FEATURE)}
    spacing = None
    for name in ("spacing", "minor"):
        row = rows.get(name)
        if row is not None:
            spacing = row["value"]
            break
    if spacing is None:
        # A backend that advertises no spacing row still draws the one world
        # grid, whose step FerroCAD.Grid owns.
        spacing = "%g mm" % FerroCAD.Grid.spacing

    return _json(
        {
            "feature": _GRID_FEATURE,
            "backend": active["backend"],
            "label": active["label"],
            "spacing": spacing,
            "providers": providers,
        }
    )


def _grid_view_request():
    """The ``{"view": …}`` request for the current grid settings.

    ``FerroCAD.Grid.apply()`` describes the native ground. A provider that draws
    the grid itself answers the refresh with the ``grid_overlay`` flag it owns
    (the Coin overlay hides the native ground while it is bound); that flag
    follows the master switch, so turning the grid off restores the native
    layer instead of leaving the two grids stacked. A provider that uses the
    native ground returns no claim, and the request keeps its default.
    """
    request = FerroCAD.Grid.apply()
    refreshed = FerroCADGui.refreshProviders(_GRID_FEATURE)
    owned = (refreshed.get("view") or {}).get("grid_overlay")
    if owned is not None:
        request["view"]["grid_overlay"] = bool(request["view"].get("grid")) and bool(
            owned
        )
    return request


def axis_summary() -> str:
    """The axis's provider and stem size for the shell's popover.

    ``backend``/``label`` name the active provider and ``length``/``width`` are
    its rows' display values with the ``px`` unit the schema leaves implicit
    (``"80.0 px"``); ``providers`` lists every registered axis backend with its
    ``active`` flag. The shape is closed: when no axis provider is registered,
    the same keys are present with ``None`` and an empty list.
    """
    manifests = FerroCADGui.providers(_AXIS_FEATURE)
    providers = [
        {
            "backend": manifest["backend"],
            "label": manifest["label"],
            "active": bool(manifest["active"]),
        }
        for manifest in manifests
    ]
    if not manifests:
        return _json(
            {
                "feature": _AXIS_FEATURE,
                "backend": None,
                "label": None,
                "length": None,
                "width": None,
                "providers": [],
            }
        )

    active = next(
        (manifest for manifest in manifests if manifest["active"]), manifests[0]
    )
    rows = {row["name"]: row for row in FerroCADGui.settings(_AXIS_FEATURE)}
    size = {}
    for name in ("length", "width"):
        row = rows.get(name)
        if row is not None:
            # The schema publishes the stems as bare floats; the label carries
            # the ``px`` unit, so the summary spells it out.
            size[name] = "%s px" % row["value"]
        else:
            # A backend that advertises no stem row still draws the one world
            # axis, whose size FerroCAD.Axis owns.
            size[name] = "%g px" % getattr(FerroCAD.Axis, name)

    return _json(
        {
            "feature": _AXIS_FEATURE,
            "backend": active["backend"],
            "label": active["label"],
            "length": size["length"],
            "width": size["width"],
            "providers": providers,
        }
    )


def provider_features() -> str:
    """Every registered provider feature, with its backends and active flag.

    This is the Dev Tools providers panel: the registry already groups by
    feature, so the shell can render one row per feature and switch through
    :func:`set_provider` without opening the mods dialog. Each backend also
    carries the ``command`` it owns (``None`` when it has none), which is what
    lets the shell disable the losing backend's command-list entry. The order is
    the registry's registration order.
    """
    features = {}
    for manifest in FerroCADGui.providers():
        features.setdefault(manifest["feature"], []).append(
            {
                "backend": manifest["backend"],
                "label": manifest["label"],
                "active": bool(manifest["active"]),
                "command": manifest["command"],
            }
        )
    return _json(
        [
            {"feature": feature, "providers": providers}
            for feature, providers in features.items()
        ]
    )


def provider_skin(payload_json: str) -> str:
    """One feature's provider-supplied WGSL source, for the shell to build.

    The payload is ``{"feature": "gizmo"}``. This is how a provider's material
    program reaches the shell: :func:`FerroCADGui.providerSkin` answers with the
    **active** backend's source, so a provider switch makes the shell ask again
    instead of drawing the outgoing skin. The result is ``{"source": "…"}``, or
    ``{"source": null}`` when no skin was supplied and the host should draw its
    native material.

    An empty or malformed payload means "no feature" and answers
    ``{"source": null}``: the shell calls this across the interpreter boundary
    and has no exception path for it.
    """
    try:
        payload = json.loads(payload_json)
        feature = str(payload.get("feature") or "")
    except Exception:  # noqa: BLE001 - a bad payload means "no feature"
        feature = ""
    source = FerroCADGui.providerSkin(feature) if feature else None
    return _json({"source": source})


def _axis_view_request():
    """The ``{"view": …}`` request for the current axis settings.

    ``FerroCAD.Axis.apply()`` describes the native triad. A provider that draws
    the axis itself answers the refresh with the ``axis.overlay`` flag it owns
    (the Coin overlay hides the native triad while it is active); that flag
    follows the master switch, so turning the axis off restores the native layer
    instead of leaving the two axes stacked. A provider that uses the native
    triad returns ``overlay=False``, and the request keeps its default.
    """
    request = FerroCAD.Axis.apply()
    refreshed = FerroCADGui.refreshProviders(_AXIS_FEATURE)
    owned = ((refreshed.get("view") or {}).get("axis") or {}).get("overlay")
    if owned is not None:
        request["view"]["axis"]["overlay"] = bool(
            request["view"]["axis"]["enabled"]
        ) and bool(owned)
    return request


def set_provider(payload_json: str) -> str:
    """Switch one feature's active provider from the shell's popover.

    The payload is ``{"feature": "grid", "backend": "coin"}``. The registry
    validates the pair, persists the choice and runs the outgoing ``deactivate``
    and incoming ``activate`` callbacks, which answer with the ``{"view": …}``
    request the shell repaints from. This bridge only marshals the request and
    folds the registry result into the command-outcome shape: ``ok``, ``error``
    (the callback errors, joined), ``status`` and ``view``.

    A malformed payload or an unknown feature/backend is reported, never raised:
    the shell calls this across the interpreter boundary and has no exception
    path for it.
    """
    try:
        payload = json.loads(payload_json)
        feature = str(payload.get("feature") or "")
        backend = str(payload.get("backend") or "")
    except Exception as exc:  # noqa: BLE001 - a bad payload is a result, not a crash
        return _json({"ok": False, "error": str(exc), "status": "", "view": {}})

    try:
        result = FerroCADGui.setActiveProvider(feature, backend)
    except Exception as exc:  # noqa: BLE001 - an unknown pair is a result too
        return _json({"ok": False, "error": str(exc), "status": "", "view": {}})

    errors = result.get("errors") or []
    return _json(
        {
            "ok": bool(result.get("ok", True)),
            "error": "; ".join(str(error) for error in errors),
            "status": str(result.get("status") or ""),
            "view": dict(result.get("view") or {}),
        }
    )


class _GridCommand:
    """``App_ToggleGrid`` — show or hide the reference grid.

    ``FerroCAD.Grid.enabled`` is the one grid switch; this command owns it and
    returns the view request derived from it. The native ground follows ``grid``
    and a provider that draws the grid itself (a shader overlay) is asked to
    re-read the switch through :func:`FerroCADGui.refreshProviders`, so the check
    mark, the store and the drawn grid cannot disagree. ``IsChecked`` reads the
    snapshot the shell mirrors back from the returned request.
    """

    def GetResources(self):
        return {
            "MenuText": "Grid",
            "ToolTip": "Show the reference grid",
            "Group": "View",
            "CmdType": "ForView",
            "FerroCAD_Surface": "grid",
        }

    def IsChecked(self):
        return bool(FerroCADGui.ViewState.grid)

    def Activated(self):
        grid = FerroCAD.Grid
        grid.enabled = not grid.enabled
        return _grid_view_request()


FreeCADGui.addCommand("App_ToggleGrid", _GridCommand())


class _AxisCrossCommand:
    """``App_ToggleAxisCross`` — show or hide the world axis cross (presentation only).

    Upstream's equivalent is ``Std_ToggleAxisCross``. It is a checkable ``view`` request
    and is **independent of the grid**: one is orientation, the other placement.
    """

    def GetResources(self):
        return {
            "MenuText": "Axis cross",
            "ToolTip": "Show the world axis cross",
            "Group": "View",
            "CmdType": "ForView",
            "FerroCAD_Surface": "axis",
        }

    def IsChecked(self):
        return bool(FerroCADGui.ViewState.axis_cross)

    def Activated(self):
        axis = getattr(FerroCAD, "Axis", None)
        if axis is None:
            # Upstream/headless builds without the semantic axis capability keep
            # the original presentation-only request.
            return {"view": {"axis_cross": not FerroCADGui.ViewState.axis_cross}}
        axis.enabled = not axis.enabled
        return _axis_view_request()


FreeCADGui.addCommand("App_ToggleAxisCross", _AxisCrossCommand())


class _ReloadAxisSettingsCommand:
    """``App_ReloadAxisSettings`` — apply the axis preferences without a restart.

    The values are read and validated by :data:`FerroCAD.Axis` from the native
    provider's group (``User parameter:BaseApp/Preferences/Mods/axis/ferrocad``),
    so this is the same request ``App_ToggleAxisCross`` and ``Settings/Axis``
    describe, including the active provider's overlay claim.
    """

    def GetResources(self):
        return {
            "MenuText": "Reload axis settings",
            "ToolTip": "Apply the axis preferences",
            "Group": "View",
            "CmdType": "ForView",
            "FerroCAD_Surface": "axis",
            "FerroCAD_Parent": "App_ToggleAxisCross",
        }

    def Activated(self):
        if getattr(FerroCAD, "Axis", None) is None:
            # Upstream/headless builds without the semantic axis capability keep
            # the original presentation-only request.
            return {"view": {"axis_cross": bool(FerroCADGui.ViewState.axis_cross)}}
        return _axis_view_request()


FreeCADGui.addCommand("App_ReloadAxisSettings", _ReloadAxisSettingsCommand())


class _AxisSettingsCommand:
    """``View_AxisSettings`` — select the ``Settings/Axis`` node.

    The axis popover's one settings row, mirroring ``View_GridSettings``. With an
    axis provider registered the provider-advertised rows are the settings
    surface, so this returns the ``select`` directive; with none it reports
    instead of opening an empty dialog, since the axis feature only exists as a
    provider.
    """

    def GetResources(self):
        return {
            "MenuText": "Axis settings",
            "ToolTip": "Edit the axis cross",
            "Group": "View",
            "CmdType": "ForView",
            "FerroCAD_Surface": "axis",
            "FerroCAD_Parent": "App_ToggleAxisCross",
        }

    def Activated(self):
        if FerroCADGui.providers(_AXIS_FEATURE):
            return {"select": _AXIS_SETTINGS_NODE}
        return {"status": "no axis provider is registered"}


FreeCADGui.addCommand("View_AxisSettings", _AxisSettingsCommand())


class _GridSettingsCommand:
    """``View_GridSettings`` — select the ``Settings/Grid`` node.

    Since S4 the provider-advertised settings are the one grid-settings surface:
    with a grid provider registered this command returns the ``select``
    directive so the shell shows that node. The modal dialog (a thin client of
    :data:`FerroCAD.Grid`) stays as the headless/upstream fallback when nothing
    is registered. A validation error stays in the modal (the callback raises
    ``ValueError`` and the facade reports it as the dialog's error).
    """

    def GetResources(self):
        return {
            "MenuText": "Grid settings",
            "ToolTip": "Change the reference grid's appearance",
            "Group": "View",
            "CmdType": "ForView",
            "FerroCAD_Surface": "grid",
            "FerroCAD_Parent": "App_ToggleGrid",
        }

    def Activated(self):
        if FerroCADGui.providers(_GRID_FEATURE):
            return {"select": _GRID_SETTINGS_NODE}
        grid = FerroCAD.Grid
        return {
            "title": "Grid settings",
            "accept": "Apply",
            "fields": [
                {
                    "name": "enabled",
                    "label": "Show grid",
                    "kind": "bool",
                    "value": grid.enabled,
                },
                {
                    "name": "color",
                    "label": "Colour",
                    "kind": "text",
                    "value": grid.color,
                },
                {
                    "name": "opacity",
                    "label": "Opacity",
                    "kind": "float",
                    "value": grid.opacity,
                    "min": 0.0,
                    "max": 1.0,
                },
                {
                    "name": "spacing",
                    "label": "Spacing (mm)",
                    "kind": "quantity",
                    "value": grid.spacing,
                    "min": 0.001,
                },
                {
                    "name": "shadows",
                    "label": "Shadows",
                    "kind": "bool",
                    "value": grid.shadows,
                },
                {
                    "name": "shadow_opacity",
                    "label": "Shadow opacity",
                    "kind": "float",
                    "value": grid.shadow_opacity,
                    "min": 0.0,
                    "max": 1.0,
                },
            ],
            "on_accept": _apply_grid_settings,
        }


def _apply_grid_settings(values):
    """Write a dialog's grid fields to ``FerroCAD.Grid`` and return its request."""
    grid = FerroCAD.Grid
    grid.enabled = bool(values.get("enabled", grid.enabled))
    grid.color = str(values.get("color", grid.color))
    grid.opacity = float(values.get("opacity", grid.opacity))
    grid.spacing = float(values.get("spacing", grid.spacing))
    grid.shadows = bool(values.get("shadows", grid.shadows))
    grid.shadow_opacity = float(values.get("shadow_opacity", grid.shadow_opacity))
    return _grid_view_request()


FreeCADGui.addCommand("View_GridSettings", _GridSettingsCommand())


class _ModsSettingsCommand:
    """``Mods_Settings`` — choose which mod backend provides a feature.

    Each registered feature becomes one ``enum`` field whose options are the
    providers' labels. Accepting calls :func:`FerroCADGui.setActiveProvider` for
    every feature that changed and returns the merged ``{"view": …}`` request,
    so the shell applies the old backend's ``deactivate`` and the new one's
    ``activate`` in the same repaint. With no registered providers it reports
    that instead of opening an empty dialog.
    """

    def GetResources(self):
        return {
            "MenuText": "Mods",
            "ToolTip": "Choose which mod provides each feature",
            "Group": "Edit",
            "CmdType": "ForEdit",
            "FerroCAD_Surface": "mods",
            "FerroCAD_DevOnly": "true",
        }

    def Activated(self):
        features = {}
        for provider in FerroCADGui.providers():
            features.setdefault(provider["feature"], []).append(provider)
        if not features:
            return {"status": "no mod providers are registered"}

        fields = []
        for feature, items in features.items():
            active = FerroCADGui.activeProvider(feature)
            current = next(
                (item for item in items if item["backend"] == active), items[0]
            )
            fields.append(
                {
                    "name": feature,
                    "label": feature.replace("_", " ").title(),
                    "kind": "enum",
                    "values": [item["label"] for item in items],
                    "value": current["label"],
                }
            )
        return {
            "title": "Mod providers",
            "accept": "Apply",
            "fields": fields,
            "on_accept": _apply_mod_settings,
        }


def _apply_mod_settings(values):
    """Switch every changed feature and return the shell's view request.

    A value may be a provider's label (what the dialog sends) or its backend id
    (what a script is likely to send); an unknown feature or value is reported,
    not fatal. Features that did not change run no callbacks.
    """
    view = {}
    changed = []
    problems = []
    for feature, choice in values.items():
        items = FerroCADGui.providers(feature)
        if not items:
            problems.append(str(feature))
            continue
        backend = None
        for item in items:
            if str(choice) in (item["label"], item["backend"]):
                backend = item["backend"]
                break
        if backend is None:
            problems.append("%s=%s" % (feature, choice))
            continue
        if backend == FerroCADGui.activeProvider(feature):
            continue
        result = FerroCADGui.setActiveProvider(feature, backend)
        view.update(result.get("view") or {})
        problems.extend(result.get("errors") or [])
        changed.append("%s: %s" % (feature, backend))

    if changed:
        status = "active provider: " + ", ".join(changed)
    elif problems:
        status = "unknown provider: " + ", ".join(problems)
    else:
        status = "no provider changed"
    if problems and changed:
        status += "; " + "; ".join(problems)
    if view:
        return {"view": view, "status": status}
    return {"status": status}


FreeCADGui.addCommand("Mods_Settings", _ModsSettingsCommand())


class _ReloadGridSettingsCommand:
    """``App_ReloadGridSettings`` — apply the grid preferences without a restart.

    The values are read and validated by :data:`FerroCAD.Grid` from the native
    provider's group (``User parameter:BaseApp/Preferences/Mods/grid/ferrocad``),
    so this is the same request ``View_GridSettings`` and ``Settings/Grid``
    describe. A radius of 0 restores the camera-derived extent.
    """

    def GetResources(self):
        return {
            "MenuText": "Reload grid settings",
            "ToolTip": "Apply the grid preferences",
            "Group": "View",
            "CmdType": "ForView",
            "FerroCAD_Surface": "grid",
            "FerroCAD_Parent": "App_ToggleGrid",
        }

    def Activated(self):
        return _grid_view_request()


FreeCADGui.addCommand("App_ReloadGridSettings", _ReloadGridSettingsCommand())


class _ActivateWorkplaneCommand:
    """``App_ActivateWorkplane`` — make the face under the pointer the workplane."""

    def GetResources(self):
        return {
            "MenuText": "Activate workplane",
            "ToolTip": "Sketch on the face under the pointer",
            "Group": "View",
            "CmdType": "ForView",
            "FerroCAD_Surface": "workplane",
            "FerroCAD_Parent": "App_ViewWorkplane",
        }

    def Activated(self):
        return {"view": {"workplane": "activate"}}


class _ClearWorkplaneCommand:
    """``App_ClearWorkplane`` — return to the world ground plane."""

    def GetResources(self):
        return {
            "MenuText": "Clear workplane",
            "ToolTip": "Return to the world frame",
            "Group": "View",
            "CmdType": "ForView",
            "FerroCAD_Surface": "workplane",
            "FerroCAD_Parent": "App_ViewWorkplane",
        }

    def Activated(self):
        return {"view": {"workplane": "clear"}}


class _ViewWorkplaneCommand:
    """``App_ViewWorkplane`` — look down the active workplane's normal."""

    def GetResources(self):
        return {
            "MenuText": "View workplane",
            "ToolTip": "Look down the active workplane's normal",
            "Group": "View",
            "CmdType": "ForView",
            "FerroCAD_Surface": "workplane",
        }

    def Activated(self):
        return {"view": {"workplane": "view"}}


FreeCADGui.addCommand("App_ActivateWorkplane", _ActivateWorkplaneCommand())
FreeCADGui.addCommand("App_ClearWorkplane", _ClearWorkplaneCommand())
FreeCADGui.addCommand("App_ViewWorkplane", _ViewWorkplaneCommand())


class _DevToggleToolsCommand:
    """``Dev_ToggleTools`` — open, focus or close the Dev Tools window.

    The GPUI shell handles the window itself (it is the only layer that can own one);
    this command is the registry entry every other path sees, and its ``Activated`` is
    the headless answer: with no GUI window there is nothing to toggle.
    """

    def GetResources(self):
        return {
            "MenuText": "Dev Tools",
            "ToolTip": "Open or close the developer tools window",
            "Accel": "Ctrl+Shift+D",
            "Group": "View",
            "CmdType": "ForView",
            "FerroCAD_Surface": "hidden",
        }

    def IsActive(self):
        return True

    def Activated(self):
        return {"status": "dev tools: no GUI window to toggle"}


FreeCADGui.addCommand("Dev_ToggleTools", _DevToggleToolsCommand())
