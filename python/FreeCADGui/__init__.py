"""The ``Gui`` module: the command registry and the document views.

In **headless** mode (``FreeCAD.GuiUp == 0``) there is no window, but
``getDocument`` / ``activeDocument`` still work: every open App document has a
``GuiDocument`` whose ``ActiveView`` owns a real, mutable ``pivy.coin`` scene
graph. The graph stays local when no GUI is up; when the GUI is up its root is
also registered with ``FerroCADGui.addOverlay`` so the shell can draw it. In the
app (``GuiUp == 1``) the ``bite-gpui`` shell supplies the chrome and consumes
the registry below.

This module is the **upstream-shaped** surface only. Everything FerroCAD adds —
the mod provider registry, provider-advertised settings, the Coin overlay
bridge and the shell's view-state mirror — lives on ``FerroCADGui`` (see
``docs/architecture/api-namespaces.md``); the two are deliberately not aliases
of each other.

What is here today:

* the **command registry** — ``addCommand`` / ``removeCommand`` /
  ``listCommands`` / ``runCommand``, with FreeCAD's ``Command`` shape
  (``Activated``, ``IsActive``, ``IsChecked``, ``GetResources``);
* the **dialog protocol** — a command's ``on_accept`` callback stays here under a
  single-use token, and ``_accept`` / ``_cancel`` are the later half of the
  non-blocking modal;
* the **document views** — ``getDocument`` / ``activeDocument`` (and the lazy
  ``ActiveDocument``) return a ``GuiDocument``; ``ActiveView.getSceneGraph``
  returns a per-document ``pivy.coin.SoSeparator`` that scripts can mutate;
* the **headless form loader** — ``PySideUic.loadUi`` returns a proxy tree from
  ``ferrocad_ui_py`` (Q2a). The extension is imported lazily, so this module
  stays Qt-free and importable before it is built;
* a **read-only selection snapshot** (``Selection.getSelection`` etc.), filled by
  the shell;
* the **headless window seam** — ``getMainWindow().getActiveWindow()`` returns
  the active document view (or a view without a scene graph), with the event
  callback slots, ``fitAll``, ``redraw`` and ``getPoint`` Draft's utilities ask
  for; ``addModule`` / ``doCommand`` carry a command's delayed work;
* the **Draft control surface** (Q4) — ``Snapper``, ``draftToolBar``, the hint
  channel and the checkable ``Draft_Snap_*`` commands, all installed by the
  ``draft`` submodule at the end of this file;
* the private bridge helpers the app script calls (``_command_list``,
  ``_run``, ``_accept``, ``_cancel``) and the document-view cache helpers.

What is still absent: ``ViewProvider`` (per-object view objects) and live view
sync (camera, fit, redraw); those are later work. See
``docs/workbenches/actions-and-tools.md`` and ``FreeCADGui/draft.py``.
"""

from __future__ import annotations

import os
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
# Documents and views
# ---------------------------------------------------------------------------

#: One live ``GuiDocument`` per open App document, keyed by document name.
_DOCUMENTS: dict = {}

#: View diagnostics already printed, so a missing Coin/shell reports once.
_REPORTED_VIEWS: set = set()


def _report_view_once(topic: str, exc: BaseException) -> None:
    if topic in _REPORTED_VIEWS:
        return
    _REPORTED_VIEWS.add(topic)
    print(f"FreeCADGui: {topic} unavailable: {_error_text(exc)}", file=sys.stderr)


def _coin_module():
    """The ``pivy.coin`` module, imported only when a scene graph is built.

    ``import FreeCADGui`` must keep working without the Coin extension, so the
    import is lazy. A missing (or broken) extension is reported once and the
    caller degrades to ``None`` instead of raising: a document view still exists
    without a scene graph.
    """
    try:
        from pivy import coin
    except Exception as exc:  # noqa: BLE001 - degrade with a diagnostic
        _report_view_once("pivy.coin", exc)
        return None
    return coin


def _document_name(name):
    """Normalize a document name or an App document to its name.

    Upstream Draft calls ``Gui.getDocument(self.doc)`` with the document object
    rather than its name, so both forms are accepted here.
    """
    if isinstance(name, str):
        return name
    candidate = getattr(name, "Name", None)
    if isinstance(candidate, str):
        return candidate
    raise TypeError("a document name or an App document is required")


def _detach_children(root) -> None:
    """Remove every child without relying on ``removeChild`` identity semantics."""
    remove_all = getattr(root, "removeAllChildren", None)
    if callable(remove_all):
        try:
            remove_all()
            return
        except Exception:  # noqa: BLE001 - fall back to one-at-a-time
            pass
    while True:
        try:
            if not root.getNumChildren():
                return
            root.removeChild(root.getChild(0))
        except Exception:  # noqa: BLE001 - nothing else to try
            return


class View3DInventor:
    """A document's 3D view, shaped like upstream's ``View3DInventor``.

    ``getSceneGraph`` is the real surface: enough for a script to attach a Coin
    node. The remaining members are the headless half of the view protocol that
    ``draftutils.gui_utils.get_3d_view`` and Draft's Snapper reach for:
    event callbacks are stored in this view, ``fitAll``/``redraw`` report that
    the request was accepted, and ``getPoint`` defers to the Snapper session
    driver. There is no camera to move, so fitting and redrawing are request
    bookkeeping rather than rendering.
    """

    def __init__(self, document: "GuiDocument") -> None:
        self._document = document
        # FreeCADGui's view protocol: event name -> callbacks.
        self._event_callbacks: dict = {}
        self._fits: int = 0
        self._redraws: int = 0

    def getSceneGraph(self):
        """The document's scene root (``None`` when Coin is unavailable)."""
        return self._document._scene_graph()

    def addEventCallback(self, name, callback) -> None:
        """Register ``callback`` for the Coin event ``name``.

        Coin is absent in a headless build, so the callbacks are kept here for
        the shell/tests to drain; no event is ever emitted by this view.
        """
        if not isinstance(name, str) or not name:
            raise ValueError("an event callback name must be a non-empty string")
        if not callable(callback):
            raise TypeError("an event callback must be callable")
        self._event_callbacks.setdefault(name, []).append(callback)

    def removeEventCallback(self, name, callback) -> bool:
        """Drop ``callback`` from ``name``; returns whether it was registered."""
        callbacks = self._event_callbacks.get(name)
        if not callbacks:
            return False
        try:
            callbacks.remove(callback)
        except ValueError:
            return False
        if not callbacks:
            del self._event_callbacks[name]
        return True

    def fitAll(self):
        """Accept a fit request (there is no camera in a headless view)."""
        self._fits += 1
        return True

    def redraw(self):
        """Accept a redraw request (the shell repaints from the scene graph)."""
        self._redraws += 1
        return True

    def getPoint(self, callback=None, *args):
        """Start a Snapper point session on this view.

        A headless view cannot receive a click, so a callback is required: the
        caller drives it with :func:`FreeCADGui.draft.pick`.
        """
        if callback is None:
            raise RuntimeError(
                "a headless view has no interactive point picker; pass a "
                "callback and drive it with FreeCADGui.draft.pick(point)"
            )
        snapper = globals().get("Snapper")
        if snapper is None:
            raise RuntimeError(
                "FreeCADGui.Snapper is unavailable; the Draft module did not load"
            )
        return snapper.getPoint(callback, *args)


class GuiDocument:
    """The GUI side of one open App document.

    ``ActiveView.getSceneGraph()`` returns a mutable ``pivy.coin.SoSeparator``
    owned by this document. The root is built lazily and cached, so every call
    returns the same node; two documents never share one.

    The root is registered with ``FerroCADGui.addOverlay`` only when
    ``FreeCAD.GuiUp`` is set. Headless callers keep a local node and never touch
    the shell bridge. ``close()`` is the ``removeObject`` / ``removeSceneGraph``
    half of a document close: it unregisters the overlay and detaches the
    children.
    """

    def __init__(self, name: str, document) -> None:
        self.Name = name
        self._document = document
        self.ActiveView = View3DInventor(self)
        self._root = None
        self._overlay = False
        self._closed = False

    def _scene_graph(self):
        if self._closed:
            return None
        if self._root is None:
            coin = _coin_module()
            if coin is None:
                return None
            try:
                self._root = coin.SoSeparator()
            except Exception as exc:  # noqa: BLE001 - report, do not crash
                _report_view_once("pivy.coin.SoSeparator", exc)
                return None
            self._register_overlay()
        return self._root

    def _register_overlay(self) -> None:
        if not getattr(FreeCAD, "GuiUp", 0):
            return
        try:
            import FerroCADGui as bridge
        except Exception as exc:  # noqa: BLE001 - headless builds have no bridge
            _report_view_once("FerroCADGui overlay bridge", exc)
            return
        add = getattr(bridge, "addOverlay", None)
        if add is None:
            _report_view_once(
                "FerroCADGui.addOverlay", AttributeError("not available")
            )
            return
        try:
            add(self._root)
        except Exception as exc:  # noqa: BLE001
            _report_view_once("FerroCADGui.addOverlay", exc)
            return
        self._overlay = True

    def _unregister_overlay(self) -> None:
        if not self._overlay:
            return
        self._overlay = False
        if self._root is None:
            return
        try:
            import FerroCADGui as bridge
        except Exception as exc:  # noqa: BLE001
            _report_view_once("FerroCADGui overlay bridge", exc)
            return
        remove = getattr(bridge, "removeOverlay", None)
        if remove is None:
            return
        try:
            remove(self._root)
        except Exception as exc:  # noqa: BLE001
            _report_view_once("FerroCADGui.removeOverlay", exc)

    def close(self) -> None:
        """Forget the view and its scene graph; safe to call more than once."""
        if self._closed:
            return
        self._unregister_overlay()
        root, self._root = self._root, None
        self._closed = True
        if root is not None:
            _detach_children(root)


def _forget_document(name) -> None:
    """Drop a cached ``GuiDocument`` (an explicit close-shaped refresh).

    Accepts a document name or an App document, so an observer can pass either.
    """
    if not isinstance(name, str):
        name = getattr(name, "Name", name)
    document = _DOCUMENTS.pop(name, None)
    if document is not None:
        document.close()


def _clear_documents() -> None:
    """Drop every cached view (used by tests and a future quit path)."""
    for name in list(_DOCUMENTS):
        _forget_document(name)


def getDocument(name):
    """The ``GuiDocument`` for an open App document, or ``None``.

    ``name`` is a document name or an App document. The App registry is the
    source of truth: a name that is not open yields ``None``, and a cached view
    is dropped once its document is gone (or a new document reused the name).
    """
    name = _document_name(name)
    document = FreeCAD.getDocument(name)
    if document is None:
        _forget_document(name)
        return None
    cached = _DOCUMENTS.get(name)
    if (
        cached is not None
        and cached._document is document
        and not cached._closed
    ):
        return cached
    if cached is not None:
        cached.close()
    view = GuiDocument(name, document)
    _DOCUMENTS[name] = view
    return view


def activeDocument():
    """The ``GuiDocument`` for ``FreeCAD.ActiveDocument``, or ``None``."""
    document = FreeCAD.ActiveDocument
    if document is None:
        return None
    return getDocument(document)


def addDocumentObserver(observer) -> None:
    """Upstream-shaped alias for :func:`FerroCADGui.addDocumentListener`.

    Upstream's GUI module has a document-observer slot, and FerroCAD implements
    it on the extension namespace so the App facade can reach it without
    importing the GUI layer. The callbacks are the upstream names:
    ``onDocumentCreated`` / ``onDocumentActivated`` / ``onDocumentDeactivated`` /
    ``onDocumentClosed``; a missing method is simply not called.
    """
    import FerroCADGui

    FerroCADGui.addDocumentListener(observer)


def removeDocumentObserver(observer) -> bool:
    """Remove a GUI document observer; returns whether it was registered."""
    import FerroCADGui

    return FerroCADGui.removeDocumentListener(observer)


# ---------------------------------------------------------------------------
# The headless window, hints and command helpers
# ---------------------------------------------------------------------------


class _NoDocument:
    """The document-shaped stand-in behind the window with no open document."""

    @staticmethod
    def _scene_graph():
        return None


#: The single headless 3D view used when no App document is open.
_HEADLESS_VIEW = View3DInventor(_NoDocument())


class _HeadlessMainWindow:
    """The small ``getMainWindow()`` object a headless GUI exposes.

    Upstream tools call ``getActiveWindow()`` for a view and occasionally probe
    for a toolbar child. The active document's real :class:`View3DInventor` is
    returned when one exists; otherwise a shared view with no scene graph is
    used, so ``gui_utils.get_3d_view()`` never has to special-case a missing
    window when ``FreeCAD.GuiUp`` is set.
    """

    Name = "FerroCAD MainWindow"
    ObjectName = "FerroCAD MainWindow"

    def getActiveWindow(self):
        document = activeDocument()
        if document is not None:
            return document.ActiveView
        return _HEADLESS_VIEW

    def getWindowsOfType(self, _type=None):
        """Every open document view (the headless analogue of the window list)."""
        return [entry.ActiveView for entry in _DOCUMENTS.values() if not entry._closed]

    def findChild(self, *_args, **_kwargs):
        """No Qt child widgets exist in a headless build."""
        return None

    def addToolBar(self, *_args, **_kwargs):
        """Accept (and ignore) a toolbar attachment; the shell owns chrome."""
        return None

    def removeToolBar(self, *_args, **_kwargs):
        """Accept (and ignore) a toolbar removal; the shell owns chrome."""
        return None

    def getObjectName(self):
        return self.ObjectName


_MAIN_WINDOW = _HeadlessMainWindow()


#: Modules recorded by :func:`addModule` for the next commit. They are names,
#: not imported objects: the shell resolves them when it builds the transaction.
_PENDING_MODULES: list = []


def addModule(name: str) -> None:
    """Record a module name for the next commit without importing it.

    Upstream ``Gui.addModule`` delays the import until the command actually
    executes. FerroCAD keeps the name here for the shell and never imports it in
    this call, so importing a tool's module cannot have side effects early.
    """
    if not isinstance(name, str) or not name:
        raise ValueError("a module name must be a non-empty string")
    if name not in _PENDING_MODULES:
        _PENDING_MODULES.append(name)


def _take_modules() -> list:
    """Consume and return the modules recorded since the last commit."""
    modules = list(_PENDING_MODULES)
    _PENDING_MODULES.clear()
    return modules


def doCommand(text: str):
    """Evaluate a Python command string in ``FreeCAD``'s namespace.

    Upstream Draft's line tools commit their work through ``Gui.doCommand``.
    When ``FreeCAD.runCommand`` exists it is used, matching upstream's host
    hook; otherwise the source is executed with ``exec`` against the ``FreeCAD``
    module's globals, so ``import Draft`` and ``FreeCAD.newDocument`` resolve as
    they do in a macro. A failure is raised as a ``RuntimeError`` that keeps the
    original exception as its cause.
    """
    if not isinstance(text, str) or not text.strip():
        raise ValueError("FreeCADGui.doCommand needs a non-empty command string")
    runner = getattr(FreeCAD, "runCommand", None)
    try:
        if callable(runner):
            return runner(text)
        exec(compile(text, "<FreeCADGui.doCommand>", "exec"), FreeCAD.__dict__)
    except Exception as exc:  # noqa: BLE001 - re-raised with the command text
        raise RuntimeError(
            f"FreeCADGui.doCommand failed: {_error_text(exc)}\n{text}"
        ) from exc
    return None


def getMainWindow():
    """The headless main window (a stable singleton).

    ``getActiveWindow()`` returns the active document's view when there is one,
    else a view with ``getSceneGraph() is None``. The view protocol includes the
    event-callback slots, ``fitAll``, ``redraw`` and ``getPoint``.
    """
    return _MAIN_WINDOW


#: Set when the Draft extension could not be installed at the end of this module.
_DRAFT_IMPORT_ERROR = None

#: Names installed by ``FreeCADGui.draft``; used for the degraded-build error.
_DRAFT_NAMES = ("draft", "Snapper", "draftToolBar", "HintManager", "InputHint", "UserInput")


def __getattr__(name):
    # PEP 562: resolved lazily because the active document changes over time.
    if name == "ActiveDocument":
        return activeDocument()
    if name in _DRAFT_NAMES and _DRAFT_IMPORT_ERROR is not None:
        raise ImportError(
            f"FreeCADGui.{name} is unavailable: the Draft GUI module failed to "
            f"import ({_error_text(_DRAFT_IMPORT_ERROR)})"
        ) from _DRAFT_IMPORT_ERROR
    raise AttributeError("module 'FreeCADGui' has no attribute '%s'" % name)


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
                "dev_only": bool(resources.get("FerroCAD_DevOnly")),
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
    JSON). ``status``, ``value``, ``view``, ``tool`` and ``select`` stay
    top-level; everything else is ignored until a later slice gives it meaning.
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
    if "select" in result:
        payload["select"] = result["select"]
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


# ---------------------------------------------------------------------------
# Qt Designer forms (Q2a)
# ---------------------------------------------------------------------------


def _import_ui_extension():
    """Import the headless form-proxy extension on first use.

    The lazy import keeps ``import FreeCADGui`` free of the extension (and of
    Qt), which is what lets the headless facade work when the native module has
    not been built. A missing module is an actionable ``ImportError``, not a
    silent no-op.
    """
    try:
        import ferrocad_ui_py
    except ImportError as exc:
        raise ImportError(
            "FreeCADGui.PySideUic.loadUi requires the `ferrocad_ui_py` "
            "extension module. Build it with `cargo build -p ferrocad_ui_py` "
            "and make the resulting library importable as `ferrocad_ui_py` "
            "(copy `libferrocad_ui_py.so` to `ferrocad_ui_py.abi3.so` in the "
            "`python/` directory, or point PYTHONPATH at it)."
        ) from exc
    return ferrocad_ui_py


def _resolve_resource(path: str):
    """Open a ``:/...`` resource through the pure-Python PySide reader."""
    from PySide.QtCore import QFile

    handle = QFile(path)
    if not handle.open(QFile.ReadOnly):
        raise OSError(f"FreeCADGui.PySideUic.loadUi: cannot open resource {path!r}")
    return handle


class _PySideUic:
    """``FreeCADGui.PySideUic``: the headless ``.ui`` loader.

    ``loadUi`` returns a proxy tree (``FormProxy``), never a window. The typed
    proxy classes live in ``ferrocad_ui_py`` and are imported on first use, so
    merely importing ``FreeCADGui`` stays free of both Qt and the extension.
    """

    @staticmethod
    def loadUi(path_or_file):
        """Load a form from a path, a ``:/`` resource or a ``QFile``-like object."""
        module = _import_ui_extension()

        if isinstance(path_or_file, (str, bytes, os.PathLike)):
            path = os.fsdecode(os.fspath(path_or_file))
            if path.startswith(":"):
                return module.load_ui(_resolve_resource(path), path)
            return module.load_ui(path)

        if hasattr(path_or_file, "readAll") or hasattr(path_or_file, "data"):
            return module.load_ui(path_or_file)

        raise TypeError(
            "FreeCADGui.PySideUic.loadUi expects a path, a ':/' resource, or a "
            "QFile-like object with readAll()/data(), got "
            f"{type(path_or_file).__name__}"
        )


PySideUic = _PySideUic()


# ---------------------------------------------------------------------------
# Draft: the native Snapper, the task-panel adapter and the snap commands
# ---------------------------------------------------------------------------

# The import is guarded: a build without the Rust stage extension must still
# import ``FreeCADGui``. ``draft`` imports ``ferrocad_stage`` lazily, so a
# missing extension surfaces as a clear ``ImportError`` from a Snapper method
# (and ``__getattr__`` re-raises this failure for the installed names).
try:
    from . import draft as draft  # noqa: F401  (installs Snapper/draftToolBar)
except Exception as _exc:  # noqa: BLE001 - keep the facade importable
    _DRAFT_IMPORT_ERROR = _exc
