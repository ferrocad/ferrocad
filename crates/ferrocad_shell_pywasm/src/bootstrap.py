"""The FreeCAD surface ``python/ferrocad_shell`` runs against, for the pywasm spike.

Four things happen here, all on the Python side of the bridge:

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
   parameter backend here, and no provider is ever registered.

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
    sys.modules[name] = module
    return module


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


def _no_providers(*_args, **_kwargs):
    """The provider registry, with nothing in it.

    ``python/FerroCADGui/__init__.py`` keeps one record per registered backend; a
    browser build registers none (there is no mod provider yet), so every question
    gets the same closed empty answer the native facade gives in that state.
    """
    return []


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
    return ("%s: %s" % (type(exc).__name__, detail)).strip()


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

    A predicate this facade cannot evaluate — `App_ToggleVisibility` reads
    `FreeCADGui.Selection`, `App_Undo` reads `Document.UndoCount`, and this half has
    neither yet (B18a step 4) — answers **enabled**. The real facade disables a command
    whose predicate raises, because there the predicate has everything it needs; here a
    raise means the state is absent, not that the command is dead. Disabling it would
    override the document half's own `enabled` (the shell merges the script's row over
    the catalogue's) and take a verb away that still works.
    """
    is_active = getattr(command, "IsActive", None)
    if is_active is None:
        return True
    try:
        return bool(is_active())
    except Exception as exc:  # noqa: BLE001 - an absent capability is not a dead command
        _report_once(name, "IsActive", exc)
        return True


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

    A command that needs a part of the contract this build has not grown yet (the
    ``FreeCADGui.Selection`` mirror, a document write) refuses with
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


def refreshProviders(feature):
    """Ask ``feature``'s active backend to re-read the shared state.

    The native facade runs the active provider's ``refresh`` callback here and
    collects its ``view`` claim. No provider is ever registered in this build
    (`providers()` is empty), so the answer is the closed empty one — no backend,
    no claim, no error — in the same shape the two request builders read.
    """
    return {
        "ok": True,
        "feature": feature,
        "backend": None,
        "view": {},
        "status": "",
        "errors": [],
    }


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
        "providers": _no_providers,
        "activeProvider": lambda feature: None,
        "refreshProviders": refreshProviders,
        "gridViewRequest": gridViewRequest,
        "axisViewRequest": axisViewRequest,
        "appearanceView": appearanceView,
        "_feature_view": _feature_view,
        "_cancel_all": _cancel_all,
        # `setActiveProvider`, `providerSkin`, `settings` and `setSetting` are
        # omitted: each needs a registered provider, and the host's `providers`
        # and `settings` contract groups are off (B18a steps 3 and 5), so no code
        # path above reaches them.
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
