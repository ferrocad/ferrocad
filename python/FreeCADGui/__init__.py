"""The ``Gui`` module: the command registry, and (later) the viewer.

In **headless** mode (``FreeCAD.GuiUp == 0``) this is a stand-in: there is no
viewer, no document view, no ``getDocument``. In the app (``GuiUp == 1``) the
``bite-gpui`` shell supplies the chrome and consumes the registry below.

What is here today:

* the **command registry** — ``addCommand`` / ``removeCommand`` /
  ``listCommands`` / ``runCommand``, with FreeCAD's ``Command`` shape
  (``Activated``, ``IsActive``, ``IsChecked``, ``GetResources``);
* the **mod provider registry** — ``registerProvider`` / ``providers`` /
  ``activeProvider`` / ``setActiveProvider``: a mod declares which *feature* a
  backend provides, exactly one backend per feature is active, and switching
  runs the outgoing ``deactivate`` and incoming ``activate`` callbacks (see
  ``docs/workbenches/mod-providers.md``);
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
(``ViewProvider``); those are M5 work. See ``docs/workbenches/actions-and-tools.md``.
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

#: Provider manifests: ``feature -> backend -> record``. Both levels keep
#: registration order, which is the deterministic tie-break for equal priorities.
_PROVIDERS: dict = {}

#: ``feature -> backend`` whose ``activate``/``deactivate`` callbacks last ran.
#: A persisted choice alone does not mean the effects were applied: the first
#: explicit switch is what runs them.
_PROVIDER_APPLIED: dict = {}

#: Monotonic registration serial (never reused), for the priority tie-break.
_PROVIDER_SERIAL: int = 0

#: ``(feature, backend, hook)`` provider callbacks already reported, so a broken
#: backend does not spam the console on every switch.
_PROVIDER_REPORTED: set = set()

#: The parameter group a mod's provider choice is persisted in.
PROVIDER_PREFERENCES = "User parameter:BaseApp/Preferences/Mods"


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
# Mod providers
#
# A mod that replaces a feature's implementation (the semantic and Coin grids
# are the first pair) declares itself from ``InitGui.py`` with
# :func:`registerProvider`. The registry keeps exactly one active backend per
# feature and switches the viewport state by calling the outgoing provider's
# ``deactivate`` and the incoming provider's ``activate``. See
# ``docs/workbenches/mod-providers.md``.
# ---------------------------------------------------------------------------


def _provider_prefs():
    """The parameter group holding one backend id per feature."""
    return FreeCAD.ParamGet(PROVIDER_PREFERENCES).GetGroup("Providers")


def _report_provider(feature, backend, hook: str, exc: BaseException) -> None:
    """Report a failing ``activate``/``deactivate`` once per provider hook."""
    key = (feature, backend, hook)
    if key in _PROVIDER_REPORTED:
        return
    _PROVIDER_REPORTED.add(key)
    print(
        f"FreeCADGui: provider {backend!r} for {feature!r} {hook} raised: "
        f"{_error_text(exc)}",
        file=sys.stderr,
    )


def registerProvider(
    feature: str,
    backend: str,
    label: str,
    command=None,
    priority: int = 0,
    activate=None,
    deactivate=None,
):
    """Declare that ``backend`` provides ``feature`` (called from ``InitGui.py``).

    ``label`` is the user-facing name the ``Mods_Settings`` dialog shows;
    ``command`` is the command id the backend owns (``None`` when it has none);
    ``priority`` decides the default backend (higher wins, earliest registration
    breaks a tie); ``activate``/``deactivate`` are optional zero-argument
    callbacks that make the backend the drawn one and undo that. ``activate``
    runs only on a **switch**, never at registration, and callback errors are
    isolated. One exception: if this registration takes the selection over (a
    higher priority, or a saved choice), the outgoing backend's ``deactivate``
    runs so a mod that drew during its own import cannot keep drawing. The
    incoming backend is still activated by the first switch.

    Registering the same ``(feature, backend)`` again replaces the record (a mod
    reload) without moving it in the registration order. Returns the manifest.
    """
    global _PROVIDER_SERIAL

    if not isinstance(feature, str) or not feature:
        raise ValueError("a provider feature must be a non-empty string")
    if not isinstance(backend, str) or not backend:
        raise ValueError("a provider backend must be a non-empty string")
    if command is not None and not isinstance(command, str):
        raise TypeError("a provider command must be a command id string or None")
    if activate is not None and not callable(activate):
        raise TypeError(f"provider {feature!r}/{backend!r} activate is not callable")
    if deactivate is not None and not callable(deactivate):
        raise TypeError(f"provider {feature!r}/{backend!r} deactivate is not callable")

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
    }
    _PROVIDER_REPORTED.difference_update(
        {key for key in _PROVIDER_REPORTED if key[0] == feature and key[1] == backend}
    )

    # A takeover is a selection change caused by registration. Stop the outgoing
    # backend, but leave the incoming one for the first explicit switch (which
    # keeps ``activate`` out of import time).
    selected = _selected(feature)
    if selected == backend and before is not None and before != backend:
        outgoing = records.get(before)
        if outgoing is not None:
            _deactivate_on_takeover(outgoing, feature)
            if _PROVIDER_APPLIED.get(feature) == before:
                _PROVIDER_APPLIED.pop(feature, None)
    return _manifest(records[backend], feature)


def _deactivate_on_takeover(record, feature) -> None:
    """Stop a backend whose feature changed hands while a mod was registering."""
    callback = record.get("deactivate")
    if callback is None:
        return
    try:
        callback()
    except Exception as exc:  # noqa: BLE001 - a broken backend is not fatal
        _report_provider(feature, record["backend"], "deactivate", exc)


def unregisterProvider(feature: str, backend: str) -> bool:
    """Drop a provider record; returns whether it existed.

    This is a reload/teardown helper and does **not** run ``deactivate``; the
    caller owns the effects (the Coin mod, for example, shuts its manager down
    before re-registering). A persisted choice is deliberately left alone, so a
    re-registered backend keeps its selection.
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
    """The backend that *is* the provider: the persisted choice, else priority.

    A persisted value that no longer names a registered backend is ignored (a
    mod was removed or renamed); the highest priority wins. An equal priority is
    broken by registration order, earliest first, so the answer is deterministic.
    """
    records = _PROVIDERS.get(feature)
    if not records:
        return None
    saved = _provider_prefs().GetString(feature, "")
    if saved in records:
        return saved
    return max(
        records.values(),
        key=lambda record: (record["priority"], -record["order"]),
    )["backend"]


def providers(feature=None) -> list:
    """The provider manifests, in registration order.

    With a ``feature``, only that feature's backends (``[]`` when none are
    registered); without one, every feature in first-registration order. Each
    entry carries ``feature``, ``backend``, ``label``, ``command``, ``priority``
    and ``active``. The callbacks stay private so the list stays JSON-safe.
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
        entries.extend(
            _manifest(record, name, active) for record in _PROVIDERS[name].values()
        )
    return entries


def activeProvider(feature):
    """The active backend for ``feature``, or ``None`` when none is registered.

    This is the declaration (the persisted choice, else the highest priority).
    A mod checks it in ``IsActive``/``Activated`` to refuse to run when another
    backend owns the feature.
    """
    return _selected(feature)


def setActiveProvider(feature: str, backend: str) -> dict:
    """Make ``backend`` the active provider for ``feature`` and apply it.

    The choice is persisted first, then the outgoing backend is deactivated and
    the incoming one activated, each at most once per change. The outgoing
    backend is the applied one when there is one, else the declared active one,
    so a first switch still restores the default it left. Selecting the
    already-declared backend runs its ``activate`` once (the bootstrap case).

    Callback errors are caught and reported (never fatal), as command callbacks
    are; they are listed under ``errors``. Returns a dict with ``ok``,
    ``feature``, ``backend``, ``previous``, the merged ``view`` request and a
    one-line ``status``. An unknown feature or backend raises ``ValueError``.
    """
    records = _PROVIDERS.get(feature)
    if not records:
        raise ValueError(f"no provider is registered for feature {feature!r}")
    if backend not in records:
        raise ValueError(f"{backend!r} does not provide feature {feature!r}")

    # The declared active backend before the choice changes. It is the outgoing
    # one even when its effects were never applied: its ``deactivate`` is an
    # idempotent "restore the default", so the first switch still leaves the
    # viewport consistent. Selecting the already-declared default is the one case
    # that still runs ``activate`` once, which is how a backend is bootstrapped.
    declared = _selected(feature)
    applied = _PROVIDER_APPLIED.get(feature)
    outgoing_backend = applied if applied is not None else declared
    _provider_prefs().SetString(feature, backend)

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

    message = f"{feature}: {backend} is the active provider"
    if statuses:
        message = "; ".join(statuses) + "; " + message
    if result["errors"]:
        message += " (reported: " + "; ".join(result["errors"]) + ")"
    result["status"] = message
    return result


def _invoke_provider(record, feature, hook, result, statuses) -> None:
    """Run one provider callback, isolating and collecting its result."""
    callback = record.get(hook)
    if callback is None:
        return
    try:
        value = callback()
    except Exception as exc:  # noqa: BLE001 - one broken backend is not fatal
        _report_provider(feature, record["backend"], hook, exc)
        result["errors"].append(f"{hook}: {_error_text(exc)}")
        return
    if not isinstance(value, dict):
        return
    view = value.get("view")
    if isinstance(view, dict):
        result["view"].update(view)
    status = value.get("status")
    if status:
        statuses.append(str(status))


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
    replaces Coin's ``getSceneGraph()``; see ``docs/workbenches/grid-overlay-plan.md`` §3.2.
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
    ``docs/workbenches/actions-and-tools.md``).
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
