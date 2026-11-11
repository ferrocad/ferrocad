"""The FreeCAD surface ``python/ferrocad_shell`` runs against, for the pywasm spike.

Six things happen here, all on the Python side of the bridge:

1. **The host gaps.** ``wasm32-unknown-unknown`` has no POSIX module, so RustPython's
   frozen ``os``/``_io`` layer is unavailable and ``contextlib``, ``io``, ``pathlib``
   and ``traceback`` cannot be imported. ``ferrocad_shell`` imports all four, so this
   file supplies the handful of names it touches. They are small but real: the console
   goes through ``evaluate()``, which captures stdout with ``io.StringIO``.
2. **The document model.** ``_ferrocad.document()`` returns the property rows the Rust
   side computed with ``ferrocad_core`` (``present::kind`` / ``present::format``),
   read from the host's *active* document each time it is asked. This file turns them
   into the ``FreeCAD.ActiveDocument`` shape the script expects, so the console sees
   the document the shell is showing — including after a file is opened — and reports
   no active document (`None`) when there is none.
3. **The Gui facade's view half** (B18a steps 1–2). ``FerroCAD.Grid``/``Axis``/``Gizmo``,
   ``FerroCADGui.ViewState`` and the two ``{"view": …}`` request builders are what the
   shell's own Gui commands (``App_ToggleGrid``, ``App_ToggleAxisCross``,
   ``View_GridSettings``, …) read, and ``FreeCADGui._command_list``/``_run`` is what
   ``ferrocad_shell.commands()``/``run_command()`` read back. They are ports of the
   native facades' *shape* over this build's state: the semantic stores have no
   parameter backend here.
4. **The selection mirror** (B18a step 4). ``FreeCADGui.Selection`` is a port of
   ``python/FreeCADGui/__init__.py``'s: the shell pushes its snapshot down
   (``ferrocad_shell.set_selection``) and every write a script makes is applied here
   *and* queued for the shell (``ferrocad_shell.take_selection_requests``), because
   Python cannot command the viewport synchronously. There is no viewport under this
   half, so nothing here ever preselects on its own — see the note by the class.
5. **The mod provider registry** (B18a step 5). ``FerroCADGui.registerProvider`` and the
   rest of ``python/FerroCADGui/__init__.py``'s provider section, so a mod's providers
   are listed by ``ferrocad_shell.provider_features`` (About, Dev Tools) and switched
   through the header popover. The selected backend is in memory (no parameter store:
   B18b), a schema's rows are read for their declared defaults (writes are step 3), and
   the native document observer has no counterpart (one live document).

The spike's honest limit: these modules are **not** the ``*_py`` PyO3 bindings. Those
are CPython extension modules and cannot be loaded by RustPython; §6.3 records that as
the fork's central trade-off. What is shared is the *script* and the *rules* — the
kinds and the display text still come from ``ferrocad_core``, not from here. The
document is also **read-only** from this side: writes, recompute and the `Shape` API
are the document half's, and a write here raises rather than silently missing it.
"""

import json
import sys

import _ferrocad


def _payload():
    """The host's active document rows, or ``None`` when no document is open."""
    return json.loads(_ferrocad.document())


def _module(name, namespace):
    """A module named ``name`` whose attributes are ``namespace``."""
    module = type(sys)(name)
    module.__dict__.update(namespace)
    module.__dict__["__name__"] = name
    # Every module built here is a substitute for a native one, so a missing
    # attribute is a capability this half has not grown — never a typo in the
    # substitute, which would be a bug in this file. `_Absent` is what lets a
    # command predicate tell the two apart (`_enabled`).
    module.__dict__["__getattr__"] = _absent_attribute(name)
    sys.modules[name] = module
    return module


class _Absent(AttributeError):
    """A facade name this build does not install (B18b).

    An ``AttributeError``, so ``hasattr``/``except AttributeError`` behave exactly as
    they would for a module that simply has no such attribute, and reported as one
    (``_reported_name``) because that is what a script sees.
    """

    _reported_name = "AttributeError"


def _absent_attribute(module_name):
    """The ``__getattr__`` a facade module raises ``_Absent`` from."""

    def __getattr__(attribute):
        raise _Absent(
            "module %r has no attribute %r: this build does not install it"
            % (module_name, attribute)
        )

    return __getattr__


# --- the modules wasm32 cannot import -------------------------------------


class _StringIO:
    """Text buffer, enough for ``ferrocad_shell.evaluate``'s stdout capture."""

    def __init__(self, initial=""):
        self._chunks = [initial]

    def write(self, text):
        self._chunks.append(str(text))
        return len(str(text))

    def writelines(self, lines):
        for line in lines:
            self.write(line)

    def getvalue(self):
        return "".join(self._chunks)

    def flush(self):
        pass

    def close(self):
        pass

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        return False


class _Suppress:
    def __init__(self, *exceptions):
        self._exceptions = exceptions or (Exception,)

    def __enter__(self):
        return self

    def __exit__(self, kind, value, tb):
        return kind is not None and issubclass(kind, self._exceptions)


class _Redirect:
    """``contextlib.redirect_stdout``/``redirect_stderr`` over ``sys``."""

    def __init__(self, attribute, target):
        self._attribute = attribute
        self._target = target

    def __enter__(self):
        self._saved = getattr(sys, self._attribute, None)
        setattr(sys, self._attribute, self._target)
        return self._target

    def __exit__(self, *exc):
        setattr(sys, self._attribute, self._saved)
        return False


class _Path:
    """The slice of ``pathlib`` the shell uses (``Path(...).name``/``is_file``)."""

    def __init__(self, *parts):
        self._text = "/".join(str(part) for part in parts)

    def __str__(self):
        return self._text

    @property
    def name(self):
        return self._text.rsplit("/", 1)[-1]

    @property
    def parent(self):
        head, _, _ = self._text.rpartition("/")
        return _Path(head or ".")

    @property
    def suffix(self):
        name = self.name
        return "" if "." not in name else "." + name.rsplit(".", 1)[-1]

    def with_name(self, name):
        """``Path(__file__).with_name(...)``, the shell's import-time catalogue read."""
        head, _, _ = self._text.rpartition("/")
        return _Path(head + "/" + name if head else name)

    def is_file(self):
        return False

    def exists(self):
        return False

    def __truediv__(self, other):
        return _Path(self._text, other)

    def __eq__(self, other):
        return isinstance(other, _Path) and str(self) == str(other)

    def __hash__(self):
        return hash(self._text)


_module(
    "io",
    {"StringIO": _StringIO, "TextIOBase": object, "BytesIO": _StringIO},
)
_module(
    "contextlib",
    {
        "suppress": _Suppress,
        "redirect_stdout": lambda target: _Redirect("stdout", target),
        "redirect_stderr": lambda target: _Redirect("stderr", target),
    },
)
_module("pathlib", {"Path": _Path, "PurePath": _Path})
_module("traceback", {"format_exc": lambda: ""})


# --- the document model ---------------------------------------------------


class DocumentObject:
    """One object, as ``ferrocad_shell.properties`` reads it."""

    def __init__(self, row):
        self._row = row

    @property
    def Name(self):
        return self._row["name"]

    @property
    def Label(self):
        return self._row["label"]

    @property
    def TypeId(self):
        return self._row["type_id"]

    @property
    def PropertiesList(self):
        return [property_row["name"] for property_row in self._row["properties"]]

    @property
    def ExpressionEngine(self):
        # Pairs of (property, expression). The spike stores no expressions, but the
        # shell asks for the list on every row, so the shape matters.
        return [(name, "") for name in self._row["driven"]]

    def _row_of(self, prop):
        for property_row in self._row["properties"]:
            if property_row["name"] == prop:
                return property_row
        raise AttributeError(prop)

    def getTypeOfProperty(self, prop):
        # The flag names the payload carries (`ferrocad_core::status_names`), which is
        # what `getTypeOfProperty` reports natively. It used to answer with the type's
        # last segment, which made the script-visible `status` row disagree with
        # CPython's.
        return self._row_of(prop)["status"]

    def getTypeIdOfProperty(self, prop):
        return self._row_of(prop)["type_id"]

    def getEnumerationsOfProperty(self, prop):
        return self._row_of(prop)["choices"]

    def __getattr__(self, prop):
        if prop.startswith("_"):
            raise AttributeError(prop)
        return self._row_of(prop)["value"]

    def __setattr__(self, name, value):
        """Read-only: the document half owns the write path.

        Silently setting an instance attribute would look like an edit and change
        nothing, so a script that tries says so instead.
        """
        if name.startswith("_"):
            object.__setattr__(self, name, value)
            return
        raise RuntimeError(
            "pywasm: the document is read-only from Python; edit it in the shell"
        )

    def __repr__(self):
        return "<DocumentObject {}>".format(self.Name)


class Document:
    def __init__(self, payload):
        self._payload = payload
        self.Name = payload["name"]
        self.Label = payload["name"]
        # FreeCAD's `UndoCount`/`RedoCount`, from the payload's counts (which
        # `ferrocad_doc_api::payload` reads from `ferrocad_core`'s transaction
        # stacks). A payload rendered before those keys existed reports no history.
        # `UndoNames`/`RedoNames` are deliberately absent: undoing is a document
        # write, and this half has no write path, so `App_Undo` refuses at the first
        # name it asks for (`UndoNames`) with an AttributeError naming it.
        self.UndoCount = int(payload.get("undo_count", 0))
        self.RedoCount = int(payload.get("redo_count", 0))
        self.Objects = [DocumentObject(row) for row in payload["objects"]]

    def getObject(self, name):
        for object_row in self.Objects:
            if object_row.Name == name:
                return object_row
        return None

    def getObjectsByLabel(self, label):
        return [row for row in self.Objects if row.Label == label]

    def __repr__(self):
        return "<Document {}>".format(self.Name)


class _LiveDocument:
    """``FreeCAD.ActiveDocument``: the host's document, as it is now.

    The document half owns the document and can replace it (boot, open, close), so
    each attribute read asks the bridge again instead of holding the rows that
    existed at import. A read forwarded here returns an object from *that* read, so a
    caller that follows one attribute to the next sees one consistent snapshot.
    """

    def _document(self):
        payload = _payload()
        if payload is None:
            raise RuntimeError("no active document")
        return Document(payload)

    def __getattr__(self, name):
        if name.startswith("__"):
            raise AttributeError(name)
        return getattr(self._document(), name)

    def __repr__(self):
        try:
            return repr(self._document())
        except RuntimeError:
            return "<no active document>"


_ACTIVE = _LiveDocument()


def _active_document():
    """The live document, or ``None`` when the host has none open."""
    try:
        _ACTIVE.Name
    except RuntimeError:
        return None
    return _ACTIVE


def _get_document(name):
    try:
        return _ACTIVE if name == _ACTIVE.Name else None
    except RuntimeError:
        return None


def _get_object(name):
    try:
        return _ACTIVE.getObject(name)
    except RuntimeError:
        return None


_module(
    "FreeCAD",
    {
        "ActiveDocument": _ACTIVE,
        "Version": lambda: ["0", "0", "0", "pywasm"],
        "backend": "pywasm",
        # The half is installed by a host that has a window (the browser module and
        # `--script-host=pywasm`); the crate's own tests boot it headlessly and read no
        # `GuiUp` branch. It is what makes `View_GridSettings` select the tree card
        # instead of opening the headless modal, which is the surface the shell draws.
        "GuiUp": 1,
        "getDocument": _get_document,
        "getObject": _get_object,
    },
)


# --- the command registry -------------------------------------------------
#
# **Ports, not a new design.** `python/FreeCADGui/__init__.py` keeps this registry
# and the `_command_list`/`_resources`/`_enabled`/`_checked`/`_run` helpers; the
# shell's own Gui commands (`App_ToggleGrid`, `App_ToggleAxisCross`,
# `View_GridSettings`, …) are registered here at import, and
# `ferrocad_shell.commands()`/`run_command()` read them back through these names.
# The only difference is what a command's `Activated` reaches: the native facade
# reads the parameter store, this file reads the semantic objects further down.

#: Commands the shell module registers at import (`FreeCADGui.addCommand('App_Move', …)`).
_COMMANDS = {}

#: ``(command, method)`` pairs already reported, so a broken command does not spam
#: the console on every refresh.
_REPORTED = set()

#: Open dialog callbacks by single-use token (``d1``, ``d2``, …). The callback cannot
#: cross the JSON boundary, so it stays here until the shell accepts or cancels it.
_DIALOGS = {}

#: Monotonic token counter (tokens are never reused, even after a cancel).
_DIALOG_SERIAL = 0

#: A tool a command asked for after its `Activated` returned (see `_request_tool`).
_pending_tool = None


def _error_text(exc):
    """``TypeError: message`` — the one line a report or a refusal carries."""
    lines = str(exc).strip().splitlines()
    detail = lines[0] if lines else ""
    # A private exception reports the builtin it is (see `_Absent`).
    name = getattr(type(exc), "_reported_name", None) or type(exc).__name__
    return ("%s: %s" % (name, detail)).strip()


def _report_once(name, method, exc):
    key = (name, method)
    if key in _REPORTED:
        return
    _REPORTED.add(key)
    print(
        "FreeCADGui: command %r %s raised: %s" % (name, method, _error_text(exc)),
        file=sys.stderr,
    )


def _name_of(command):
    for name, candidate in _COMMANDS.items():
        if candidate is command:
            return name
    return repr(command)


def _resources(command):
    get = getattr(command, "GetResources", None)
    if get is None:
        return {}
    try:
        resources = get()
    except Exception as exc:  # noqa: BLE001 - isolated on purpose
        _report_once(_name_of(command), "GetResources", exc)
        return {}
    return resources if isinstance(resources, dict) else {}


def _enabled(name, command):
    """Whether a command's `IsActive` says it can act.

    The real facade disables a command whose predicate raises, and so does this one
    now that the state the built-in predicates read exists: the selection mirror
    (`FreeCADGui.Selection`, pushed by `set_selection`) and the document's history
    (`Document.UndoCount`/`RedoCount`, from the payload). `App_Delete`, `App_Move`,
    `App_Rotate` and `App_ToggleVisibility` are judged by the same snapshot a click
    fills; `App_Undo`/`App_Redo` by the document's stacks, and a document that is not
    open raises instead of answering — which the real facade's `is not None` guard
    turns into the same `false`.

    The one exception is a *capability* this half has not grown (B18b): a predicate
    that asks a facade module for a name it does not install raises `_Absent`, and
    that answers **enabled**. Disabling the row would override the document half's
    own `enabled` (the shell merges the script's row over the catalogue's) and take
    away a verb the window can still run: an absent capability is not a dead
    command, while a predicate that is merely broken is.
    """
    is_active = getattr(command, "IsActive", None)
    if is_active is None:
        return True
    try:
        return bool(is_active())
    except _Absent as exc:
        # An absent capability, not a dead command: leave the row to the document half.
        _report_once(name, "IsActive", exc)
        return True
    except Exception as exc:  # noqa: BLE001 - a broken predicate disables one command
        _report_once(name, "IsActive", exc)
        return False


def _checked(name, command):
    is_checked = getattr(command, "IsChecked", None)
    if is_checked is None:
        return False
    try:
        return bool(is_checked())
    except Exception as exc:  # noqa: BLE001
        _report_once(name, "IsChecked", exc)
        return False


def _command_list():
    """Every command with its resources and live state, for the shell's bar."""
    commands = []
    for name, command in _COMMANDS.items():
        resources = _resources(command)
        commands.append(
            {
                "id": name,
                "menu": str(resources.get("MenuText") or name),
                "tooltip": str(resources.get("ToolTip") or ""),
                "accel": str(resources.get("Accel") or ""),
                "group": str(resources.get("Group") or ""),
                "cmd_type": str(resources.get("CmdType") or ""),
                "enabled": _enabled(name, command),
                "checked": _checked(name, command),
                "dev_only": bool(resources.get("FerroCAD_DevOnly")),
            }
        )
    return commands


def _register_dialog(callback):
    """Keep a dialog callback and return its single-use token."""
    global _DIALOG_SERIAL
    _DIALOG_SERIAL += 1
    token = "d%d" % _DIALOG_SERIAL
    _DIALOGS[token] = callback
    return token


def _command_payload(result):
    """Shape a command's return value for the shell (``FreeCADGui._payload``)."""
    payload = {"ok": True}
    if not isinstance(result, dict):
        if result is not None:
            payload["value"] = repr(result)
        return payload
    # A command may refuse (`{"ok": False, "error": …}`) as well as report a status.
    if "ok" in result:
        payload["ok"] = bool(result["ok"])
    if "error" in result:
        payload["error"] = str(result["error"])
    callback = result.get("on_accept")
    if callback is not None:
        payload["dialog"] = {
            key: result[key]
            for key in (
                "title",
                "fields",
                "accept",
                "cancel",
                "presentation",
                "preview",
            )
            if key in result
        }
        payload["token"] = _register_dialog(callback)
    for key in ("status", "value", "view", "tool", "select", "gizmo"):
        if key in result:
            payload[key] = result[key]
    if "workplane_stem" in result:
        payload["workplane_stem"] = bool(result["workplane_stem"])
    return payload


def _request_tool(tool_id):
    """Ask the shell to start ``tool_id`` after ``Activated`` returns."""
    global _pending_tool
    _pending_tool = str(tool_id)
    return None


def _run(name):
    """Invoke a command for the shell; never raises.

    A command that needs a part of the contract this build has not grown yet (a
    document write, a manifest-declared provider) refuses with
    ``{"ok": False, "error": …}`` naming it, instead of taking the command list
    down: the shell's list and palette keep working, and the refusal is what B6's
    matrix records.
    """
    global _pending_tool
    command = _COMMANDS.get(name)
    if command is None:
        return {"ok": False, "error": "unknown command %r" % name}
    _pending_tool = None
    try:
        result = command.Activated()
    except Exception as exc:  # noqa: BLE001 - one broken command is not fatal
        return {"ok": False, "error": _error_text(exc)}
    payload = _command_payload(result)
    pending = _pending_tool
    _pending_tool = None
    if pending is not None:
        payload.setdefault("tool", pending)
    return payload


def _accept(token, values):
    """Run an open dialog's callback and drop the token (single use)."""
    callback = _DIALOGS.pop(token, None)
    if callback is None:
        return {"ok": False, "error": "no open dialog %r" % token}
    try:
        result = callback(values)
    except Exception as exc:  # noqa: BLE001 - a broken callback is reported
        return {"ok": False, "error": _error_text(exc)}
    return _command_payload(result)


def _cancel(token):
    """Drop a dialog's token; a no-op when it is already gone."""
    _DIALOGS.pop(token, None)
    return {"ok": True}


def _cancel_all():
    """Drop every open dialog (document close, boot, quit)."""
    global _pending_tool
    _DIALOGS.clear()
    _pending_tool = None


def runCommand(name, *args):
    """Invoke ``name`` and return whatever ``Activated`` returned.

    Arguments are forwarded, as upstream does; an unknown id raises ``KeyError``.
    """
    command = _COMMANDS.get(name)
    if command is None:
        raise KeyError(name)
    return command.Activated(*args)


# --- the selection mirror -------------------------------------------------
#
# **A port of `python/FreeCADGui/__init__.py`'s `Selection`** (B18a step 4), not a
# design of its own. The shell owns the selection: it pushes its snapshot down
# through `ferrocad_shell.set_selection` (`_set`) after every viewport change, and
# each write a script makes here is applied to that snapshot **and** queued for the
# shell (`_take_requests`), which `ferrocad_shell.take_selection_requests` drains.
# Python cannot command the viewport synchronously, and pretending the viewport had
# changed would be a lie.
#
# The one place a script notices the missing view: an entry's ``Object`` is the plain
# document object, with no ``ViewObject`` (this facade installs no per-object view
# objects), and **nothing here ever preselects on its own** — ``getPreselection()``
# only answers what a script set, because the pointer hover that would set it lives
# in a viewport this half does not have.


class SelectionObject:
    """One selected object, shaped like upstream's ``Gui.SelectionObject``.

    ``Object`` is the document object itself, ``SubElementNames`` the picked
    sub-elements (``["Face6"]``; empty for the whole object), ``PickedPoints`` the
    world points the picks landed on (as far as the source knew them), and
    ``ObjectName`` the object's internal name. It unpacks as
    ``(Object, SubElementNames)``, so code written against the older tuple snapshot
    keeps working.
    """

    Object = None
    SubElementNames = None
    PickedPoints = None
    ObjectName = None

    def __init__(self, object, subelement_names=None, picked_points=None):
        self.Object = object
        self.SubElementNames = [str(name) for name in (subelement_names or [])]
        self.PickedPoints = list(picked_points or [])
        name = object if isinstance(object, str) else getattr(object, "Name", "")
        self.ObjectName = name if isinstance(name, str) else ""

    @property
    def HasSubObjects(self):
        """Whether the selection names sub-elements (upstream's alias)."""
        return bool(self.SubElementNames)

    def __iter__(self):
        return iter((self.Object, self.SubElementNames))

    def __len__(self):
        return 2

    def __getitem__(self, index):
        return (self.Object, self.SubElementNames)[index]

    def __eq__(self, other):
        if isinstance(other, SelectionObject):
            return (
                self.Object == other.Object
                and self.SubElementNames == other.SubElementNames
                and self.PickedPoints == other.PickedPoints
            )
        return NotImplemented

    def __repr__(self):
        return "SelectionObject(%r, %r)" % (self.ObjectName, self.SubElementNames)


class _Selection:
    """What the shell has selected, in upstream's shape.

    An entry is ``(name, [sub-elements], [picked points])``; the pushed snapshot
    carries no picked points, so a shell-made selection reports an empty
    ``PickedPoints`` unless the writer supplied one.
    """

    def __init__(self):
        self._entries = []
        self._preselection = None
        self._requests = []

    # -- reads --------------------------------------------------------------

    def _document(self, doc=None):
        # `FreeCAD.ActiveDocument` is the live proxy here and is never `None`, so the
        # question the native facade asks with it is `_active_document()`'s.
        if doc is None:
            return _active_document()
        if isinstance(doc, str):
            return _get_document(doc)
        return doc

    def _objects(self, doc=None, object_name=None):
        document = self._document(doc)
        if document is None:
            return []
        resolved = []
        for name, elements, points in self._entries:
            if object_name is not None and name != object_name:
                continue
            obj = document.getObject(name)
            if obj is not None:
                resolved.append(SelectionObject(obj, elements, points))
        return resolved

    def getSelection(self, doc=None):
        """The selected document objects (upstream ``Gui.Selection``)."""
        return [item.Object for item in self._objects(doc)]

    def getSelectionEx(self, object_name=None, doc=None):
        """The selection as :class:`SelectionObject` items, unpackable as pairs."""
        return self._objects(doc, object_name)

    def isSelected(self, obj, subname=None):
        """Whether ``obj`` (or an object name) is in the selection."""
        name = obj if isinstance(obj, str) else getattr(obj, "Name", None)
        for candidate, elements, _ in self._entries:
            if candidate != name:
                continue
            if subname is None or not elements or subname in elements:
                return True
        return False

    # -- writes (applied here, queued for the shell) ------------------------

    def addSelection(self, obj, subname=None, point=None, doc=None):
        """Add ``obj`` (with optional sub-elements) to the selection.

        An object the document does not have is ignored and reported as ``False``:
        the facade never invents an object.
        """
        document = self._document(doc)
        name = obj if isinstance(obj, str) else getattr(obj, "Name", None)
        if document is None or not name or document.getObject(name) is None:
            return False
        elements = _names(subname)
        points = _points(point)
        for index, entry in enumerate(self._entries):
            if entry[0] != name:
                continue
            merged = list(entry[1])
            merged.extend(element for element in elements if element not in merged)
            if not elements:
                merged = []  # a whole-object add wins over a stale sub-element list
            self._entries[index] = (name, merged, list(entry[2]) + points)
            break
        else:
            self._entries.append((name, elements, points))
        self._requests.append(
            {"op": "add", "object": name, "elements": elements, "points": points}
        )
        return True

    def removeSelection(self, obj, subname=None, doc=None):
        """Remove ``obj`` (or just ``subname``) from the selection."""
        del doc
        name = obj if isinstance(obj, str) else getattr(obj, "Name", None)
        elements = _names(subname)
        changed = False
        for index, (candidate, current, points) in enumerate(list(self._entries)):
            if candidate != name:
                continue
            if not elements:
                self._entries.pop(index)
                changed = True
            else:
                remaining = [element for element in current if element not in elements]
                if remaining != current:
                    self._entries[index] = (candidate, remaining, points)
                    changed = True
            break
        if changed:
            self._requests.append(
                {"op": "remove", "object": name, "elements": elements, "points": []}
            )
        return changed

    def clearSelection(self, doc=None):
        """Clear the selection (``doc`` limits it to one document's objects)."""
        if doc is None:
            self._entries = []
        else:
            document = self._document(doc)
            names = {obj.Name for obj in (getattr(document, "Objects", None) or [])}
            self._entries = [entry for entry in self._entries if entry[0] not in names]
        self._requests.append({"op": "clear", "object": "", "elements": [], "points": []})

    def setPreselection(self, obj, subname=None, point=None):
        """Set the preselection (the object the pointer is over)."""
        name = obj if isinstance(obj, str) else getattr(obj, "Name", None)
        self._preselection = (name, _names(subname), _points(point))
        self._requests.append(
            {
                "op": "preselect",
                "object": name or "",
                "elements": self._preselection[1],
                "points": self._preselection[2],
            }
        )

    def getPreselection(self):
        """The preselection as a :class:`SelectionObject`, or ``None``."""
        if self._preselection is None:
            return None
        name, elements, points = self._preselection
        document = _active_document()
        obj = document.getObject(name) if document is not None and name else None
        if obj is None:
            return None
        return SelectionObject(obj, elements, points)

    # -- the shell bridge ---------------------------------------------------

    def _set(self, pairs):
        """Replace the snapshot (called by `ferrocad_shell.set_selection`).

        Accepts ``(name, elements)`` pairs or ``(name, elements, points)`` triples.
        Writes made here are **not** queued: the snapshot is the shell's answer, not a
        request back to it.
        """
        entries = []
        for entry in pairs:
            name, elements = entry[0], entry[1]
            points = entry[2] if len(entry) > 2 else None
            entries.append((str(name), _names(elements), _points(points)))
        self._entries = entries
        self._preselection = None

    def _take_requests(self):
        """Consume the queued writes for the shell to apply to the viewport."""
        requests, self._requests = self._requests, []
        return requests


def _names(subname):
    """A sub-element selection as a list of names (``None`` → the whole object)."""
    if subname is None:
        return []
    if isinstance(subname, str):
        return [subname] if subname else []
    return [str(name) for name in subname if str(name)]


def _points(point):
    """A picked point (or a list of them) in the caller's own vector type."""
    if point is None:
        return []
    if isinstance(point, (str, bytes)):
        raise TypeError("a picked point must be a world point, not a string")
    if hasattr(point, "x") and hasattr(point, "y") and hasattr(point, "z"):
        return [point]
    try:
        return [candidate for candidate in point]
    except TypeError:
        return [point]


Selection = _Selection()


# --- the mod provider registry ---------------------------------------------
#
# **A port of `python/FerroCADGui/__init__.py`'s provider section** (B18a step 5):
# registration, the active backend per feature, the switch list the header popover and
# the `Mods_Settings` dialog read, the skin channel, and the activate/deactivate/refresh
# callbacks. `ferrocad_shell.provider_features` is what the About card and Dev Tools
# render, so with this in place a mod's providers are named by the shell.
#
# Only what this build has no store for differs (B18b):
#
# * **The selected backend is in memory.** The native registry persists one id per
#   feature in `FreeCAD.ParamGet`; here the choice lasts for the process, so a page
#   reload starts from priority again.
# * **A declared settings schema is kept and read for its declared defaults.** The rows
#   are B18a step 3: `FerroCADGui.settings` exists so a provider-backed
#   `grid_summary`/`axis_summary` can find its spacing row, but there is no store to
#   write to, so a row shows its default and editing is step 3.
# * **No document observer.** The native half attaches applied providers to document
#   activation/close; this half has one live document and receives no such callbacks.

_PROVIDERS = {}
_PROVIDER_SELECTED = {}
_PROVIDER_APPLIED = {}
_PROVIDER_SERIAL = 0
_PROVIDER_REPORTED = set()


def _report_provider(feature, backend, hook, exc):
    """Report a failing provider callback once per hook, like `_report_once`."""
    key = (feature, backend, hook)
    if key in _PROVIDER_REPORTED:
        return
    _PROVIDER_REPORTED.add(key)
    print(
        "FerroCADGui: provider %r for %r %s raised: %s"
        % (backend, feature, hook, _error_text(exc)),
        file=sys.stderr,
    )


def _manifest(record, feature, active=None):
    """One provider as JSON-safe data (the callbacks never leave this module)."""
    if active is None:
        active = _selected(feature)
    return {
        "feature": feature,
        "backend": record["backend"],
        "label": record["label"],
        "command": record["command"],
        "priority": record["priority"],
        "active": record["backend"] == active,
    }


def _selected(feature):
    """The backend that *is* the provider: the in-memory choice, else priority.

    A choice that names no registered backend is ignored (a mod was removed or
    renamed); the highest priority wins, and an equal priority is broken by
    registration order, earliest first, so the answer is deterministic.
    """
    records = _PROVIDERS.get(feature)
    if not records:
        return None
    saved = _PROVIDER_SELECTED.get(feature)
    if saved in records:
        return saved
    return max(
        records.values(),
        key=lambda record: (record["priority"], -record["order"]),
    )["backend"]


def registerProvider(
    feature,
    backend,
    label,
    command=None,
    priority=0,
    activate=None,
    deactivate=None,
    refresh=None,
    settings=None,
    skin=None,
):
    """Declare that ``backend`` provides ``feature`` (called from ``InitGui.py``).

    The ported contract, in short: ``label`` is the name the dialog shows, ``command``
    the id the backend owns, ``priority`` decides the default, and the three callbacks
    run only when asked. Registering the same ``(feature, backend)`` replaces the record
    without moving it in the order.

    ``settings`` is accepted and stored, and :func:`settings` reads its rows with the
    declared defaults (no parameter store: B18b); writing a value is step 3. ``skin`` is
    the optional WGSL source the host asks for through :func:`providerSkin`; supplying
    none means the native material.
    """
    global _PROVIDER_SERIAL

    if not isinstance(feature, str) or not feature:
        raise ValueError("a provider feature must be a non-empty string")
    if not isinstance(backend, str) or not backend:
        raise ValueError("a provider backend must be a non-empty string")
    if command is not None and not isinstance(command, str):
        raise TypeError("a provider command must be a command id string or None")
    for hook, callback in (
        ("activate", activate),
        ("deactivate", deactivate),
        ("refresh", refresh),
    ):
        if callback is not None and not callable(callback):
            raise TypeError("provider %r/%r %s is not callable" % (feature, backend, hook))
    if skin is not None and not isinstance(skin, str):
        raise TypeError(
            "provider %r/%r skin must be WGSL source text" % (feature, backend)
        )
    source = None if skin is None else skin.strip()
    if source == "":
        raise ValueError("provider %r/%r skin is empty" % (feature, backend))

    text = str(label if label is not None else backend).strip()
    records = _PROVIDERS.get(feature)
    if records is None:
        records = _PROVIDERS.setdefault(feature, {})
    before = _selected(feature)
    existing = records.get(backend)
    order = existing["order"] if existing is not None else _PROVIDER_SERIAL
    if existing is None:
        _PROVIDER_SERIAL += 1
    records[backend] = {
        "feature": feature,
        "backend": backend,
        "label": text or backend,
        "command": command,
        "priority": int(priority),
        "order": order,
        "activate": activate,
        "deactivate": deactivate,
        "refresh": refresh,
        # Kept for step 3; nothing here reads a row yet.
        "settings": list(settings) if settings is not None else [],
        "skin": source,
    }
    _PROVIDER_REPORTED.difference_update(
        {key for key in _PROVIDER_REPORTED if key[0] == feature and key[1] == backend}
    )

    # A takeover is a selection change caused by registration. Stop the outgoing
    # backend, but leave the incoming one for the first explicit switch (which keeps
    # `activate` out of import time).
    selected = _selected(feature)
    if selected == backend and before is not None and before != backend:
        outgoing = records.get(before)
        if outgoing is not None:
            _deactivate_on_takeover(outgoing, feature)
            if _PROVIDER_APPLIED.get(feature) == before:
                _PROVIDER_APPLIED.pop(feature, None)
    return _manifest(records[backend], feature)


def unregisterProvider(feature, backend):
    """Drop a provider record; returns whether it existed.

    A reload/teardown helper: it does **not** run ``deactivate``, and the in-memory
    choice is left alone so a re-registered backend keeps it.
    """
    records = _PROVIDERS.get(feature)
    if not records or backend not in records:
        return False
    del records[backend]
    if not records:
        _PROVIDERS.pop(feature, None)
    if _PROVIDER_APPLIED.get(feature) == backend:
        _PROVIDER_APPLIED.pop(feature, None)
    _PROVIDER_REPORTED.difference_update(
        {key for key in _PROVIDER_REPORTED if key[0] == feature and key[1] == backend}
    )
    return True


def providers(feature=None):
    """The provider manifests, in registration order.

    With a ``feature``, only that feature's backends (``[]`` when none are
    registered); without one, every feature in first-registration order. Each entry
    carries ``feature``, ``backend``, ``label``, ``command``, ``priority`` and
    ``active``; the callbacks stay private so the list stays JSON-safe.
    """
    if feature is not None:
        records = _PROVIDERS.get(feature)
        if not records:
            return []
        active = _selected(feature)
        return [_manifest(record, feature, active) for record in records.values()]
    entries = []
    for name in _PROVIDERS:
        active = _selected(name)
        entries.extend(_manifest(record, name, active) for record in _PROVIDERS[name].values())
    return entries


def activeProvider(feature):
    """The active backend for ``feature``, or ``None`` when none is registered."""
    return _selected(feature)


def providerSkin(feature, backend=None):
    """The WGSL material program of ``feature``'s backend, or ``None``.

    ``backend=None`` — the form the shell uses — means the **active** backend, so a
    switch makes the host ask again. ``None`` also means "draw the native material":
    an unknown feature or backend, or a backend that supplied no skin.
    """
    records = _PROVIDERS.get(feature)
    if not records:
        return None
    if backend is None:
        backend = _selected(feature)
    record = records.get(backend)
    if record is None:
        return None
    return record.get("skin")


def setActiveProvider(feature, backend):
    """Make ``backend`` the active provider for ``feature`` and apply it.

    The choice is kept in memory (no parameter store here), then the outgoing backend
    is deactivated and the incoming one activated, each at most once per change.
    Callback errors are caught and reported; an unknown feature or backend raises
    ``ValueError``.
    """
    records = _PROVIDERS.get(feature)
    if not records:
        raise ValueError("no provider is registered for feature %r" % (feature,))
    if backend not in records:
        raise ValueError("%r does not provide feature %r" % (backend, feature))

    declared = _selected(feature)
    applied = _PROVIDER_APPLIED.get(feature)
    outgoing_backend = applied if applied is not None else declared
    _PROVIDER_SELECTED[feature] = backend

    result = {
        "ok": True,
        "feature": feature,
        "backend": backend,
        "previous": declared,
        "view": {},
        "status": "",
        "errors": [],
    }
    statuses = []
    if outgoing_backend != backend:
        outgoing = records.get(outgoing_backend) if outgoing_backend is not None else None
        if outgoing is not None:
            _invoke_provider(outgoing, feature, "deactivate", result, statuses)
        _invoke_provider(records[backend], feature, "activate", result, statuses)
    elif applied is None:
        _invoke_provider(records[backend], feature, "activate", result, statuses)
    _PROVIDER_APPLIED[feature] = backend

    message = "%s: %s is the active provider" % (feature, backend)
    if statuses:
        message = "; ".join(statuses) + "; " + message
    if result["errors"]:
        message += " (reported: " + "; ".join(result["errors"]) + ")"
    result["status"] = message
    return result


def refreshProviders(feature):
    """Ask the active backend of ``feature`` to re-read the shared state.

    Only the active backend runs; an absent callback (or no provider) is a no-op. A
    callback may return the same dict a command may return, so ``view`` carries a
    request the caller can merge. Never raises for a backend problem.
    """
    result = {
        "ok": True,
        "feature": feature,
        "backend": None,
        "view": {},
        "status": "",
        "errors": [],
    }
    records = _PROVIDERS.get(feature)
    if not records:
        return result
    backend = _selected(feature)
    result["backend"] = backend
    statuses = []
    _invoke_provider(records[backend], feature, "refresh", result, statuses)
    if statuses:
        result["status"] = "; ".join(statuses)
    return result


def _deactivate_on_takeover(record, feature):
    """Stop a backend whose feature changed hands while a mod was registering."""
    callback = record.get("deactivate")
    if callback is None:
        return
    try:
        callback()
    except Exception as exc:  # noqa: BLE001 - a broken backend is not fatal
        _report_provider(feature, record["backend"], "deactivate", exc)


def _invoke_provider(record, feature, hook, result, statuses):
    """Run one provider callback, isolating and collecting its result."""
    callback = record.get(hook)
    if callback is None:
        return
    try:
        value = callback()
    except Exception as exc:  # noqa: BLE001 - one broken backend is not fatal
        _report_provider(feature, record["backend"], hook, exc)
        result["errors"].append("%s: %s" % (hook, _error_text(exc)))
        return
    if not isinstance(value, dict):
        return
    view = value.get("view")
    if isinstance(view, dict):
        result["view"].update(view)
    status = value.get("status")
    if status:
        statuses.append(str(status))


# The row kinds a schema may declare, and the type id each renders as. An unknown kind
# is allowed and read-only, exactly as in the native registry.
_SETTING_KINDS = ("text", "bool", "int", "float", "quantity", "enum", "color")
_SETTING_TYPES = {
    "text": "App::PropertyString",
    "bool": "App::PropertyBool",
    "int": "App::PropertyInteger",
    "float": "App::PropertyFloat",
    "quantity": "App::PropertyQuantity",
    "enum": "App::PropertyEnumeration",
    "color": "App::PropertyString",
}
_SETTING_DEFAULTS = {
    "text": "",
    "bool": False,
    "int": 0,
    "float": 0.0,
    "quantity": 0.0,
    "color": "#000000",
}
_SETTING_REPORTED = set()


def _report_setting(feature, backend, name, exc):
    """Report a broken schema row once per setting."""
    key = (feature, backend, name)
    if key in _SETTING_REPORTED:
        return
    _SETTING_REPORTED.add(key)
    print(
        "FerroCADGui: setting %r of provider %r for %r raised: %s"
        % (name, backend, feature, _error_text(exc)),
        file=sys.stderr,
    )


def _display_setting(spec, value):
    """The row's display string (``"10 mm"`` for a quantity).

    The native facade formats through ``FreeCAD.Units.Quantity``; this build has no
    `Units` module, so a quantity's default is shown as ``<value> <unit>``. That is the
    same text for every value a schema declares plainly, which is what a store-less
    build is showing anyway.
    """
    if spec.get("kind") != "quantity":
        return str(value)
    unit = spec.get("unit")
    if not unit:
        return str(value)
    try:
        return "%g %s" % (float(value), unit)
    except (TypeError, ValueError):
        return str(value)


def settings(feature, backend=None):
    """The declared settings rows of ``feature``'s active provider (or ``backend``).

    The rows use the property pane's contract — ``name``, ``label``, ``type``,
    ``value``, ``status``, ``editable``, ``kind``, plus ``unit``/``min``/``max`` and
    ``choices`` where the schema declares them. **There is no parameter store here**
    (B18b), so a row without a ``get`` hook shows its declared default and nothing is
    written; storing a value (``setSetting``) is B18a step 3. The summary builders call
    this for a provider-declared spacing row, which is why it exists at step 5.

    ``[]`` means "nothing to show": an unknown feature or backend, no provider at all,
    a provider that advertises no schema, or a row this build cannot read.
    """
    records = _PROVIDERS.get(feature)
    if not records:
        return []
    if backend is None:
        backend = _selected(feature)
    record = records.get(backend)
    if record is None:
        return []
    schema = record.get("settings") or []
    rows = []
    for spec in schema:
        if not isinstance(spec, dict):
            continue
        name = spec.get("name")
        kind = spec.get("kind")
        if not isinstance(name, str) or not name or not isinstance(kind, str) or not kind:
            continue
        default = spec.get("default", _SETTING_DEFAULTS.get(kind, ""))
        status = "unsupported kind" if kind not in _SETTING_KINDS else "provider"
        raw = default
        getter = spec.get("get")
        if callable(getter):
            try:
                raw = getter()
                status = "computed"
            except Exception as exc:  # noqa: BLE001 - one broken knob is not fatal
                _report_setting(feature, backend, name, exc)
                raw = default
                status = "error"
        row = {
            "name": name,
            "label": str(spec.get("label") or name),
            "type": _SETTING_TYPES.get(kind, ""),
            "value": _display_setting(spec, raw),
            "status": status,
            # The native rule is `kind in _SETTING_KINDS and (has_set or not has_get)`,
            # which is true for a row that writes to the parameter store. There is no
            # store here, so no row is editable until step 3 installs one; claiming
            # otherwise would offer an edit that cannot persist.
            "editable": False,
            "kind": kind,
        }
        if "unit" in spec:
            row["unit"] = spec["unit"]
        if "min" in spec:
            row["min"] = spec["min"]
        if "max" in spec:
            row["max"] = spec["max"]
        if "choices" in spec:
            row["choices"] = list(spec["choices"])
        rows.append(row)
    return rows


_module(
    "FreeCADGui",
    {
        # The registry under upstream's private spelling: `ferrocad_shell.commands()`
        # reads the surface/parent/icon metadata through `_COMMANDS` + `_resources`.
        "_COMMANDS": _COMMANDS,
        "addCommand": lambda name, command: _COMMANDS.setdefault(name, command),
        "removeCommand": lambda name: _COMMANDS.pop(name, None),
        "listCommands": lambda: list(_COMMANDS),
        "runCommand": runCommand,
        # The shell's selection, pushed down and written back (see the mirror above).
        "Selection": Selection,
        "SelectionObject": SelectionObject,
        "_command_list": _command_list,
        "_resources": _resources,
        "_enabled": _enabled,
        "_checked": _checked,
        "_run": _run,
        "_payload": _command_payload,
        "_request_tool": _request_tool,
        "_accept": _accept,
        "_cancel": _cancel,
        "_cancel_all": _cancel_all,
        "updateGui": lambda: None,
        "activeDocument": _active_document,
        "ActiveDocument": _ACTIVE,
        "SendMsgToActiveView": lambda *_args: None,
        "getMainWindow": lambda: None,
    },
)


# --- the view state and the semantic stores -------------------------------
#
# **There is no parameter store here.** The native modules
# (`python/FerroCAD/_grid.py`, `_axis.py`, `_gizmo.py`) read and write
# ``FreeCAD.ParamGet("User parameter:BaseApp/Preferences/Mods/<feature>/ferrocad")``,
# and this file installs no `ParamGet` (that is B18b). So a field this process never
# wrote falls back to its default — or, for a field the shell mirrors, to
# ``FerroCADGui.ViewState``, which is the only presentation state a build without a
# store has. A written field stays in the object, so `App_ToggleGrid` survives
# across console calls. A setter validates what the real one validates, so a bad
# dialog value is reported instead of being drawn.


class _ViewState:
    """``FerroCADGui.ViewState``: the shell's presentation state, pushed down.

    `ferrocad_shell.set_view_state` assigns every field (the grid switch, the axis
    cross, the projection, the grid's steps, style, widths and radius) and a
    checkable command reads them in ``IsChecked``. The object is a module-level
    singleton, so a push outlives the console call that made it.
    """

    grid = False
    axis_cross = True
    projection = "persp"
    grid_minor = 10.0
    grid_major = 100.0
    grid_adaptive = True
    grid_minor_style = "dots"
    grid_major_width = 1.0
    grid_minor_width = 1.0
    grid_radius = None


ViewState = _ViewState()


def _number(value, name, minimum=0.0, maximum=None):
    """Validate a finite number in ``minimum..maximum``; never clamp it."""
    try:
        number = float(value)
    except (TypeError, ValueError):
        raise ValueError("%s must be a number, got %r" % (name, value))
    if number != number:
        raise ValueError("%s must be finite, got %r" % (name, value))
    if number < minimum:
        raise ValueError("%s must be at least %s, got %r" % (name, minimum, value))
    if maximum is not None and number > maximum:
        raise ValueError("%s must be at most %s, got %r" % (name, maximum, value))
    return number


def _colour(value, name="a colour"):
    """A ``#rgb``/``#rrggbb`` colour, canonicalised to lowercase ``#rrggbb``."""
    if not isinstance(value, str):
        raise ValueError("%s must be a '#rrggbb' string, got %r" % (name, value))
    text = value.strip()
    digits = text[1:] if text.startswith("#") else ""
    if len(digits) == 3:
        digits = digits[0] * 2 + digits[1] * 2 + digits[2] * 2
    if len(digits) != 6 or any(
        digit not in "0123456789abcdefABCDEF" for digit in digits
    ):
        raise ValueError("%s must be #rgb or #rrggbb, got %r" % (name, text))
    return "#" + digits.lower()


class _Semantic:
    """One ``FerroCAD`` capability over an in-process dict (see the note above)."""

    #: The fields a request carries, with the defaults the real module declares.
    DEFAULTS = {}
    #: The ``#rrggbb`` fields.
    COLOURS = ()
    #: The boolean fields.
    FLAGS = ()
    #: The numeric fields, with the bounds their real setters enforce.
    BOUNDS = {}

    def __init__(self):
        self._values = {}

    def __getattr__(self, name):
        defaults = type(self).DEFAULTS
        if name not in defaults:
            raise AttributeError(name)
        if name in self._values:
            return self._values[name]
        return self._current(name, defaults[name])

    def __setattr__(self, name, value):
        if name.startswith("_"):
            object.__setattr__(self, name, value)
            return
        if name not in type(self).DEFAULTS:
            raise AttributeError(name)
        self._write(name, value)

    def _current(self, name, default):
        """The value a field has when this process never wrote it."""
        return default

    def _write(self, name, value):
        """Store one validated field."""
        self._values[name] = self._coerce(name, value)

    def _coerce(self, name, value):
        if name in type(self).FLAGS:
            return bool(value)
        if name in type(self).COLOURS:
            return _colour(value, "the " + name)
        if name in type(self).BOUNDS:
            minimum, maximum = type(self).BOUNDS[name]
            return _number(value, "the " + name, minimum=minimum, maximum=maximum)
        raise AttributeError(name)


class _Grid(_Semantic):
    """``FerroCAD.Grid`` — ``python/FerroCAD/_grid.py`` without the store."""

    DEFAULTS = {
        "enabled": False,
        "color": "#94a3b8",
        "opacity": 1.0,
        "adaptive": True,
        "minor_style": "dots",
        "major_width": 1.0,
        "minor_width": 1.0,
        "minor": 10.0,
        "major": 100.0,
        "spacing": 10.0,
        "radius": 0.0,
        "shadows": False,
        "shadow_opacity": 0.30,
    }
    COLOURS = ("color",)
    FLAGS = ("enabled", "adaptive", "shadows")
    BOUNDS = {
        "opacity": (0.0, 1.0),
        "major_width": (1.0, 8.0),
        "minor_width": (1.0, 8.0),
        "radius": (0.0, None),
        "shadow_opacity": (0.0, 1.0),
    }

    def _current(self, name, default):
        # The shell mirrors the grid it is drawing into `ViewState`; with no store
        # that mirror is the only state there is, so an unwritten field reads it.
        if name == "enabled":
            return bool(ViewState.grid)
        if name == "minor":
            return float(ViewState.grid_minor)
        if name == "major":
            return float(ViewState.grid_major)
        if name == "adaptive":
            return bool(ViewState.grid_adaptive)
        if name == "minor_style":
            return str(ViewState.grid_minor_style)
        if name == "major_width":
            return float(ViewState.grid_major_width)
        if name == "minor_width":
            return float(ViewState.grid_minor_width)
        if name == "radius":
            return 0.0 if ViewState.grid_radius is None else float(ViewState.grid_radius)
        if name == "spacing":
            return self.minor
        return default

    def _write(self, name, value):
        # `spacing` is an alias over the pair, and the pair is always a decade
        # apart: both rules are the real setters'.
        if name == "spacing":
            step = _number(value, "the grid spacing", minimum=0.000001)
            self._values["minor"] = step
            self._values["major"] = step * 10.0
            return
        if name == "minor":
            step = _number(value, "the grid minor step", minimum=0.000001)
            self._values["minor"] = step
            if step >= self.major:
                self._values["major"] = step * 10.0
            return
        if name == "major":
            step = _number(value, "the grid major step", minimum=0.000001)
            minor = self.minor
            if step < minor:
                raise ValueError(
                    "the grid major step (%s) cannot be below the minor one (%s)"
                    % (step, minor)
                )
            self._values["major"] = step
            if step == minor:
                self._values["minor"] = step / 10.0
            return
        self._values[name] = self._coerce(name, value)

    def apply(self):
        """The ``{"view": …}`` request for the current grid policy."""
        minor = self.minor
        major = self.major
        if major < minor:
            major = minor * 10.0
            self._values["major"] = major
        elif major == minor:
            minor = major / 10.0
            self._values["minor"] = minor
        return {
            "view": {
                "grid": self.enabled,
                "grid_minor": minor,
                "grid_major": major,
                "grid_adaptive": self.adaptive,
                "grid_minor_style": self.minor_style,
                "grid_major_width": self.major_width,
                "grid_minor_width": self.minor_width,
                "grid_radius": self.radius,
                "grid_color": self.color,
                "grid_alpha": self.opacity,
            }
        }


class _Axis(_Semantic):
    """``FerroCAD.Axis`` — ``python/FerroCAD/_axis.py`` without the store."""

    DEFAULTS = {
        "enabled": True,
        "color_x": "#f87171",
        "color_y": "#4ade80",
        "color_z": "#38bdf8",
        "opacity": 0.65,
        "length": 80.0,
        "width": 1.5,
        "origin": True,
        "labels": True,
        "ghost_alpha": 0.10,
    }
    COLOURS = ("color_x", "color_y", "color_z")
    FLAGS = ("enabled", "origin", "labels")
    BOUNDS = {
        "opacity": (0.0, 1.0),
        "length": (8.0, 400.0),
        "width": (0.5, 6.0),
        "ghost_alpha": (0.0, 1.0),
    }

    def _current(self, name, default):
        # The axis cross is presentation, and its flag is the one the shell mirrors.
        if name == "enabled":
            return bool(ViewState.axis_cross)
        return default

    def apply(self):
        """The ``{"view": …}`` request for the current axis policy."""
        return {
            "view": {
                "axis": {
                    "enabled": self.enabled,
                    "color_x": self.color_x,
                    "color_y": self.color_y,
                    "color_z": self.color_z,
                    "opacity": self.opacity,
                    "length": self.length,
                    "width": self.width,
                    "origin": self.origin,
                    "labels": self.labels,
                    "ghost_alpha": self.ghost_alpha,
                }
            }
        }


class _Gizmo(_Semantic):
    """``FerroCAD.Gizmo`` — ``python/FerroCAD/_gizmo.py`` without the store."""

    DEFAULTS = {
        "enabled": True,
        "active_color": "#38bdf8",
        "inactive_color": "#94a3b8",
        "outline_color": "#0f172a",
        "size": 110.0,
        "width": 2.0,
        "halo_width": 1.0,
        "hover_scale": 1.5,
        "hit_radius_px": 12.0,
        "opacity": 1.0,
        "ghost_alpha": 0.20,
        "ring_snap_deg": 15.0,
        "show_readout": True,
    }
    COLOURS = ("active_color", "inactive_color", "outline_color")
    FLAGS = ("enabled", "show_readout")
    BOUNDS = {
        "size": (8.0, 400.0),
        "width": (0.5, 6.0),
        "halo_width": (0.0, 8.0),
        "hover_scale": (1.0, None),
        "hit_radius_px": (12.0, None),
        "opacity": (0.0, 1.0),
        # The real setter clamps an occluded handle to half alpha and refuses
        # anything above it, so an occluded handle cannot read as a front one.
        "ghost_alpha": (0.0, 0.5),
        "ring_snap_deg": (0.0, 180.0),
    }

    def apply(self):
        """The ``{"view": …}`` request for the current gizmo policy."""
        return {
            "view": {
                "gizmo": {
                    "enabled": self.enabled,
                    # No provider registry here, so the native backend draws it
                    # (`_gizmo._backend()` with no active provider).
                    "backend": "ferrocad",
                    "active_color": self.active_color,
                    "inactive_color": self.inactive_color,
                    "outline_color": self.outline_color,
                    "size": self.size,
                    "width": self.width,
                    "halo_width": self.halo_width,
                    "hover_scale": self.hover_scale,
                    "hit_radius_px": self.hit_radius_px,
                    "opacity": self.opacity,
                    "ghost_alpha": self.ghost_alpha,
                    "ring_snap_deg": self.ring_snap_deg,
                    "show_readout": self.show_readout,
                }
            }
        }


FerroCAD = _module(
    "FerroCAD",
    {
        "Grid": _Grid(),
        "Axis": _Axis(),
        "Gizmo": _Gizmo(),
    },
)


#: ``feature -> the FerroCAD attribute that applies it`` — the appearance
#: capabilities this build has, in the order ``appearanceView`` merges them (the
#: native facade's ``_APPEARANCE_FACADES``/``_APPEARANCE_FEATURES``).
_APPEARANCE_FACADES = {"gizmo": "Gizmo", "axis": "Axis", "grid": "Grid"}


def _feature_view(feature):
    """One capability's current ``{"view": …}`` request, or ``None``."""
    attribute = _APPEARANCE_FACADES.get(feature)
    if attribute is None:
        return None
    return getattr(FerroCAD, attribute).apply()


def gridViewRequest():
    """The ``{"view": …}`` request for the current grid policy.

    ``FerroCAD.Grid.apply()`` describes the native ground. A provider that draws
    the grid itself answers :func:`refreshProviders` with the ``grid_overlay``
    claim it owns; with no provider registered there is no claim, so the request
    is the facade's own. That is the same composition
    ``python/FerroCADGui/__init__.py`` produces over an empty registry.
    """
    request = _feature_view("grid")
    if request is None:
        return {"view": {}}
    refreshed = refreshProviders("grid")
    owned = (refreshed.get("view") or {}).get("grid_overlay")
    if owned is not None:
        request["view"]["grid_overlay"] = bool(request["view"].get("grid")) and bool(
            owned
        )
    return request


def axisViewRequest():
    """The ``{"view": …}`` request for the current axis policy.

    ``FerroCAD.Axis.apply()`` describes the native triad; a provider that draws
    the axis itself would answer :func:`refreshProviders` with ``axis.overlay``.
    With no provider registered there is no claim, so the request is the
    facade's own.
    """
    request = _feature_view("axis")
    if request is None:
        return {"view": {}}
    refreshed = refreshProviders("axis")
    owned = ((refreshed.get("view") or {}).get("axis") or {}).get("overlay")
    if owned is not None:
        request["view"]["axis"]["overlay"] = bool(
            request["view"]["axis"]["enabled"]
        ) and bool(owned)
    return request


def appearanceView(feature=None):
    """The merged ``{"view": …}`` request for the current appearance policy.

    The native boot hook installs the edition's declared presets first; nothing
    here declares one (there is no ``registerAppearancePreset`` in this build), so
    this only merges the capabilities' own requests and collects their errors
    rather than raising across the interpreter boundary.
    """
    if feature is None:
        features = list(_APPEARANCE_FACADES)
    else:
        features = [feature]
    view = {}
    errors = []
    for name in features:
        try:
            request = _feature_view(name)
        except Exception as exc:  # noqa: BLE001 - one capability is not fatal
            errors.append("%s: %s" % (name, _error_text(exc)))
            continue
        if isinstance(request, dict):
            view.update(request.get("view") or {})
    return {"view": view, "errors": errors}


_module(
    "FerroCADGui",
    {
        "ViewState": ViewState,
        # The mod provider registry (B18a step 5): the ported
        # `python/FerroCADGui/__init__.py` section. `setSetting` is step 3 — there is
        # no store to write to yet.
        "registerProvider": registerProvider,
        "unregisterProvider": unregisterProvider,
        "providers": providers,
        "activeProvider": activeProvider,
        "providerSkin": providerSkin,
        "setActiveProvider": setActiveProvider,
        "refreshProviders": refreshProviders,
        "settings": settings,
        "gridViewRequest": gridViewRequest,
        "axisViewRequest": axisViewRequest,
        "appearanceView": appearanceView,
        "_feature_view": _feature_view,
        "_cancel_all": _cancel_all,
    },
)


# --- the engine's property rules, over the snapshot -----------------------


def _property_kind_of(obj, prop):
    return obj._row_of(prop)["kind"]


def _format_property(obj, prop):
    return obj._row_of(prop)["value"]


def _coerce_property_text(obj, prop, text):
    """The spike accepts the text verbatim: coercion is the engine's, and the
    engine is not reachable from RustPython (see the module docstring)."""
    return str(text)


_module(
    "ferrocad",
    {
        "propertyKindOf": _property_kind_of,
        "formatProperty": _format_property,
        "coercePropertyText": _coerce_property_text,
    },
)
