"""The ``Gui`` module: the command registry, and (later) the viewer.

In **headless** mode (``FreeCAD.GuiUp == 0``) this is a stand-in: there is no
viewer, no document view, no ``getDocument``. In the app (``GuiUp == 1``) the
``bite-gpui`` shell supplies the chrome and consumes the registry below.

What is here today:

* the **command registry** — ``addCommand`` / ``removeCommand`` /
  ``listCommands`` / ``runCommand``, with FreeCAD's ``Command`` shape
  (``Activated``, ``IsActive``, ``IsChecked``, ``GetResources``);
* the **dialog protocol** — a command's ``on_accept`` callback stays here under a
  single-use token, and ``_accept`` / ``_cancel`` are the later half of the
  non-blocking modal;
* a **read-only selection snapshot** (``Selection.getSelection`` etc.), filled by
  the shell;
* a **read-only view state** (``ViewState.grid``, ``ViewState.projection``), filled
  by the shell for a checkable command's ``IsChecked``;
* the private bridge helpers the app script calls (``_command_list``,
  ``_run``, ``_accept``, ``_cancel``).

What is deliberately absent: ``activeDocument()`` and view objects
(``ViewProvider``); those are M5 work. See ``docs/actions-and-tools.md``.
"""

from __future__ import annotations

import sys

import FreeCAD

__version__ = "0.1.0"

#: Registered commands, in registration order.
_COMMANDS: dict = {}

#: ``(command, method)`` pairs already reported, so a broken command does not
#: spam the console on every refresh.
_REPORTED: set = set()

#: Open dialog callbacks by single-use token (``d1``, ``d2``, …). The callback
#: cannot cross the JSON boundary, so it stays here until the shell accepts or
#: cancels the dialog.
_DIALOGS: dict = {}

#: Monotonic token counter (tokens are never reused, even after a cancel).
_DIALOG_SERIAL: int = 0


def _error_text(exc: BaseException) -> str:
    message = str(exc).strip().splitlines()
    detail = message[0] if message else ""
    return f"{type(exc).__name__}: {detail}".strip()


def _report_once(name: str, method: str, exc: BaseException) -> None:
    key = (name, method)
    if key in _REPORTED:
        return
    _REPORTED.add(key)
    print(f"FreeCADGui: command {name!r} {method} raised: {_error_text(exc)}", file=sys.stderr)


# ---------------------------------------------------------------------------
# The registry
# ---------------------------------------------------------------------------


def addCommand(name: str, command) -> None:
    """Register ``command`` under ``name`` (``"<Workbench>_<Action>"``).

    A duplicate id raises: a silent overwrite would hide workbench collisions.
    """
    if not isinstance(name, str) or not name:
        raise ValueError("a command name must be a non-empty string")
    if not callable(getattr(command, "Activated", None)):
        raise TypeError(f"command {name!r} has no callable Activated")
    if name in _COMMANDS:
        raise ValueError(f"command {name!r} is already registered")
    _COMMANDS[name] = command


def removeCommand(name: str) -> None:
    """Drop ``name`` from the registry (a no-op when absent)."""
    _COMMANDS.pop(name, None)
    _REPORTED.difference_update({key for key in _REPORTED if key[0] == name})


def listCommands() -> list:
    """The registered command ids, in registration order."""
    return list(_COMMANDS)


def runCommand(name: str, *args):
    """Invoke ``name`` and return whatever ``Activated`` returned.

    Arguments are forwarded, as upstream does; a command that takes none raises
    if the caller passes some. An unknown id raises ``KeyError``.
    """
    command = _COMMANDS.get(name)
    if command is None:
        raise KeyError(name)
    return command.Activated(*args)


# ---------------------------------------------------------------------------
# Pull-based overlays (G5)
# ---------------------------------------------------------------------------


def _coin_module():
    """The ``pivy.coin`` module, imported only when an overlay is used.

    Headless builds and the GUI facade itself do not need Coin, so this import is
    deliberately lazy: ``import FreeCADGui`` keeps working without the extension.
    """
    try:
        from pivy import coin
    except ImportError as exc:  # noqa: BLE001 - re-raised with the contract
        raise RuntimeError(
            "overlays need the Coin facade (pivy.coin) in this build"
        ) from exc
    return coin


def addOverlay(node) -> None:
    """Register a Coin node as a viewport overlay (FerroCAD extension).

    The node is **extracted now**: the shell keeps a copy of its subtree and draws
    it beside the document scene on the next refresh. This is the pull bridge that
    replaces Coin's ``getSceneGraph()``; see ``docs/grid-overlay-plan.md`` §3.2.
    A later in-place edit does not reach the copy, so an author re-registers the
    node (``removeOverlay`` then ``addOverlay``) after changing it.
    """
    return _coin_module().addOverlay(node)


def removeOverlay(node) -> bool:
    """Drop a previously registered overlay, keyed by Python object identity."""
    return _coin_module().removeOverlay(node)


# ---------------------------------------------------------------------------
# Enablement, checked state, and the shell bridge
# ---------------------------------------------------------------------------


def _resources(command) -> dict:
    get = getattr(command, "GetResources", None)
    if get is None:
        return {}
    try:
        resources = get()
    except Exception as exc:  # noqa: BLE001 - isolated on purpose
        _report_once(_name_of(command), "GetResources", exc)
        return {}
    return resources if isinstance(resources, dict) else {}


def _name_of(command) -> str:
    for name, candidate in _COMMANDS.items():
        if candidate is command:
            return name
    return repr(command)


def _enabled(name: str, command) -> bool:
    is_active = getattr(command, "IsActive", None)
    if is_active is None:
        return True
    try:
        return bool(is_active())
    except Exception as exc:  # noqa: BLE001 - a broken predicate disables one command
        _report_once(name, "IsActive", exc)
        return False


def _checked(name: str, command) -> bool:
    is_checked = getattr(command, "IsChecked", None)
    if is_checked is None:
        return False
    try:
        return bool(is_checked())
    except Exception as exc:  # noqa: BLE001
        _report_once(name, "IsChecked", exc)
        return False


def _command_list() -> list:
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
            }
        )
    return commands


def _run(name: str) -> dict:
    """Invoke a command for the shell; never raises.

    The returned dict always has ``ok``; a command may add ``status``,
    ``value``, or a ``dialog`` spec with a single-use ``token`` (see
    ``docs/actions-and-tools.md``).
    """
    command = _COMMANDS.get(name)
    if command is None:
        return {"ok": False, "error": f"unknown command {name!r}"}
    try:
        result = command.Activated()
    except Exception as exc:  # noqa: BLE001 - one broken command is not fatal
        return {"ok": False, "error": _error_text(exc)}
    return _payload(result)


def _register_dialog(callback) -> str:
    """Keep a dialog callback and return its single-use token."""
    global _DIALOG_SERIAL
    _DIALOG_SERIAL += 1
    token = f"d{_DIALOG_SERIAL}"
    _DIALOGS[token] = callback
    return token


def _payload(result) -> dict:
    """Shape a command's return value for the shell.

    A dict with ``on_accept`` is a dialog spec: the callback stays here under a
    token, and the spec goes out under ``dialog`` (the callable must never reach
    JSON). ``status`` and ``value`` stay top-level; everything else is ignored
    until a later slice gives it meaning.
    """
    payload = {"ok": True}
    if not isinstance(result, dict):
        if result is not None:
            payload["value"] = repr(result)
        return payload
    callback = result.get("on_accept")
    if callback is not None:
        payload["dialog"] = {
            key: result[key]
            for key in ("title", "fields", "accept", "cancel")
            if key in result
        }
        payload["token"] = _register_dialog(callback)
    if "status" in result:
        payload["status"] = result["status"]
    if "value" in result:
        payload["value"] = result["value"]
    if "view" in result:
        payload["view"] = result["view"]
    if "tool" in result:
        payload["tool"] = result["tool"]
    return payload


def _accept(token: str, values: dict) -> dict:
    """Run an open dialog's callback and drop the token (single use)."""
    callback = _DIALOGS.pop(token, None)
    if callback is None:
        return {"ok": False, "error": f"no open dialog {token!r}"}
    try:
        result = callback(values)
    except Exception as exc:  # noqa: BLE001 - a broken callback is reported
        return {"ok": False, "error": _error_text(exc)}
    return _payload(result)


def _cancel(token: str) -> dict:
    """Drop a dialog's token; a no-op when it is already gone."""
    _DIALOGS.pop(token, None)
    return {"ok": True}


def _cancel_all() -> None:
    """Drop every open dialog (document close, boot, quit)."""
    _DIALOGS.clear()


# ---------------------------------------------------------------------------
# Selection (read-only; writes arrive with the multi-selection service)
# ---------------------------------------------------------------------------


class _Selection:
    """A snapshot of what the shell has selected.

    The facade keeps no document state; this is UI state, pushed by the shell
    after each viewport selection. Entries are ``(object name, [subelements])``.
    """

    def __init__(self) -> None:
        self._pairs: list = []

    def _objects(self) -> list:
        doc = FreeCAD.ActiveDocument
        if doc is None:
            return []
        resolved = []
        for name, elements in self._pairs:
            obj = doc.getObject(name)
            if obj is not None:
                resolved.append((obj, list(elements)))
        return resolved

    def getSelection(self) -> list:
        """The selected document objects (upstream ``Gui.Selection``)."""
        return [obj for obj, _ in self._objects()]

    def getSelectionEx(self) -> list:
        """``[(object, [subelement names])]`` for the current selection."""
        return self._objects()

    def isSelected(self, obj) -> bool:
        """Whether ``obj`` (or an object name) is in the selection."""
        name = obj if isinstance(obj, str) else getattr(obj, "Name", None)
        return any(candidate == name for candidate, _ in self._pairs)

    def _set(self, pairs) -> None:
        """Replace the snapshot (called by the shell bridge)."""
        self._pairs = [
            (str(name), [str(element) for element in (elements or [])])
            for name, elements in pairs
        ]


Selection = _Selection()


class _ViewState:
    """The shell's presentation state, pushed by the shell (read-only here).

    ``grid`` is the reference-grid flag, ``axis_cross`` the world triad's, ``projection``
    is ``"persp"`` or ``"ortho"``, ``grid_minor``/``grid_major`` are the world grid's
    steps (mm) and ``grid_radius`` is a fixed extent (mm, or ``None`` for the
    camera-derived one). A checkable command reads these in ``IsChecked``, which is what
    makes a ``{"view": …}`` request close the loop.
    """

    grid = False
    axis_cross = True
    projection = "persp"
    grid_minor = 10.0
    grid_major = 100.0
    grid_radius = None


ViewState = _ViewState()
