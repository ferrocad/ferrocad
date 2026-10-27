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
* the **view facade** (S4.1/S4.3) — ``ActiveView`` answers the calls a canvas-first
  workbench makes: ``getObjectsInfo``/``getObjectInfo`` (a ranked, occlusion-ordered
  pick with the sub-element name), ``getPointOnScreen``, ``getViewDirection``,
  ``getCameraNode``, ``getSize``, ``addEventCallbackPivy``/``removeEventCallbackPivy``
  and ``updateGui``. The camera and stage are **published** by the shell into
  ``ferrocad_stage`` (``set_view_camera``/``set_view_stage``) and read back here, so
  the answers follow the live view rather than a frozen default;
* the **headless form loader** — ``PySideUic.loadUi`` returns a proxy tree from
  ``ferrocad_ui_py`` (Q2a). The extension is imported lazily, so this module
  stays Qt-free and importable before it is built;
* the **selection** — ``Selection.getSelection`` / ``getSelectionEx`` read the
  shell's snapshot, each entry a ``SelectionObject`` with ``Object``,
  ``SubElementNames``, ``PickedPoints`` and ``ObjectName``; the writable half
  (``addSelection``/``removeSelection``/``clearSelection``/``setPreselection``)
  updates the snapshot and queues the operation, which the shell drains into its
  own selection (so a scripted selection arms the same manipulators a click does);
* the **transient previews** (S4.6) — ``Preview`` collects unpickable
  ``ferrocad_stage.ShapeItem``s and publishes them with
  ``ferrocad_stage.set_preview_items``; the shell draws them beside the document
  stage, and the ranked picker never reports one;
* the **headless window seam** — ``getMainWindow().getActiveWindow()`` returns
  the active document view (or a view without a scene graph), with the event
  callback slots, ``fitAll``, ``redraw`` and ``getPoint`` Draft's utilities ask
  for; ``addModule`` / ``doCommand`` carry a command's delayed work;
* the **Draft control surface** (Q4) — ``Snapper``, ``draftToolBar``, the hint
  channel and the checkable ``Draft_Snap_*`` commands, all installed by the
  ``draft`` submodule at the end of this file;
* the private bridge helpers the app script calls (``_command_list``,
  ``_run``, ``_accept``, ``_cancel``) and the document-view cache helpers.

What is still absent: ``ViewProvider`` (per-object view objects). The live
``Selection`` write is now drained by the shell at every Python entry point
(``Shell::drain_selection_requests``), and the camera is republished whenever the
viewport notifies the shell (a wheel zoom or an orbit), not only on a render. A
script's ``Selection.addSelection`` therefore reaches the viewport, and
``getCameraNode`` answers with the camera the last gesture produced.
See ``docs/workbenches/actions-and-tools.md`` and ``FreeCADGui/draft.py``.
"""

from __future__ import annotations

import os
import sys

import FreeCAD

__version__ = "0.1.0"

#: Registered commands, in registration order.
_COMMANDS: dict = {}

#: A tool id a command asked the shell to start from inside ``Activated``.
#:
#: Upstream commands return ``None``; keeping that return shape while still
#: letting a command request an interactive tool is done with this side channel.
#: ``_run`` consumes it (and clears it first, so a direct ``Activated`` call in a
#: test cannot leak into an unrelated command).
_pending_tool = None

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


def _stage_module():
    """The ``ferrocad_stage`` extension, imported only when a view is asked a question.

    The shell publishes the live camera and stage there (``set_view_camera`` /
    ``set_view_stage``) and this module reads them back. ``import FreeCADGui`` must
    keep working without the extension, so the import is lazy and a missing module
    degrades to "no publication yet" instead of raising.
    """
    try:
        import ferrocad_stage
    except Exception as exc:  # noqa: BLE001 - degrade with a diagnostic
        _report_view_once("ferrocad_stage", exc)
        return None
    return ferrocad_stage


def _published_camera():
    """The camera the shell (or a script) last published, or ``None``."""
    module = _stage_module()
    reader = getattr(module, "view_camera", None)
    if reader is None:
        return None
    try:
        return reader()
    except Exception as exc:  # noqa: BLE001 - a broken publication is not a crash
        _report_view_once("ferrocad_stage.view_camera", exc)
        return None


def _triple(point) -> tuple:
    """``(x, y, z)`` from a Vector, an ``SbVec3f``-shaped value or a sequence."""
    getter = getattr(point, "getValue", None)
    if callable(getter):
        point = getter()
    if isinstance(point, (tuple, list)):
        values = point
    else:
        values = (point.x, point.y, point.z)
    if len(values) != 3:
        raise ValueError("a world point needs three coordinates")
    return (float(values[0]), float(values[1]), float(values[2]))


class _Vec3:
    """A read-only ``SbVec3f``-shaped triple.

    Coin's ``getValue()`` is the spelling every workbench uses (``V(*near.getValue())``),
    so it is here; the sequence and vector operators are for tools that skip it.
    """

    __slots__ = ("x", "y", "z")

    def __init__(self, x, y, z):
        self.x, self.y, self.z = float(x), float(y), float(z)

    def getValue(self):
        return (self.x, self.y, self.z)

    def __iter__(self):
        return iter((self.x, self.y, self.z))

    def __len__(self):
        return 3

    def __getitem__(self, index):
        return (self.x, self.y, self.z)[index]

    def __sub__(self, other):
        x, y, z = _triple(other)
        return _Vec3(self.x - x, self.y - y, self.z - z)

    def __repr__(self):
        return f"({self.x:g}, {self.y:g}, {self.z:g})"


class _Quaternion:
    """A read-only ``SbRotation``-shaped value (``getValue()`` → ``(x, y, z, w)``)."""

    __slots__ = ("_value",)

    def __init__(self, values):
        self._value = tuple(float(value) for value in values)

    def getValue(self):
        return self._value

    def multVec(self, vector):
        """Rotate ``vector`` by this quaternion (Coin's ``SbRotation.multVec``).

        ``box_selection`` derives the view's screen-right axis with
        ``camera.orientation.getValue().multVec(coin.SbVec3f(1, 0, 0))``, so the
        rotation has to be applied here rather than left to the caller.
        """
        x, y, z, w = self._value
        vx, vy, vz = _triple(vector)
        # t = 2 · (q × v); v' = v + w·t + q × t
        tx = 2.0 * (y * vz - z * vy)
        ty = 2.0 * (z * vx - x * vz)
        tz = 2.0 * (x * vy - y * vx)
        return _Vec3(
            vx + w * tx + (y * tz - z * ty),
            vy + w * ty + (z * tx - x * tz),
            vz + w * tz + (x * ty - y * tx),
        )

    def __iter__(self):
        return iter(self._value)

    def __repr__(self):
        return "(%s)" % ", ".join(f"{value:g}" for value in self._value)


class _Field:
    """A read-only Coin field: ``getValue()`` returns the published value.

    The camera belongs to the shell, so ``setValue`` is refused with a message that
    names the honest route (the shell's view state) instead of silently doing nothing.
    """

    __slots__ = ("_name", "_value")

    def __init__(self, name, value):
        self._name = name
        self._value = value

    def getValue(self):
        return self._value

    def setValue(self, *_args):
        raise RuntimeError(
            f"the view camera is owned by the shell; camera.{self._name} is read-only here"
        )

    def __repr__(self):
        return f"Field({self._name!r})"


class _CameraType:
    """A Coin ``SoType``-shaped camera type, compared by name.

    ``isOfType`` must answer for the type id a caller got from ``pivy.coin`` (which
    may be the real Coin or FerroCAD's facade), so the comparison is by name and
    every camera type is also an ``SoCamera``.
    """

    __slots__ = ("_name",)

    def __init__(self, name):
        self._name = name

    def getName(self):
        return self._name

    def isDerivedFrom(self, other):
        name = _type_key(other)
        return name == _type_key(self._name) or name == "Camera"

    def __eq__(self, other):
        return _type_key(other) == _type_key(self._name)

    def __hash__(self):
        return hash(_type_key(self._name))

    def __repr__(self):
        return f"SoType({self._name})"


def _type_name(type_id) -> str:
    """The Coin type name of ``type_id`` (a string, an SoType or anything named)."""
    getter = getattr(type_id, "getName", None)
    if callable(getter):
        try:
            return str(getter())
        except Exception:  # noqa: BLE001 - a foreign type object
            pass
    return str(type_id)


def _type_key(type_id) -> str:
    """A type name normalised for comparison.

    Real Coin's ``SoType.getName()`` returns the *registered* name, which omits the
    ``So`` prefix (``coin.SoPerspectiveCamera.getClassTypeId().getName()`` is
    ``"PerspectiveCamera"``), while FerroCAD's facade (and a class's own name) keeps
    it. Comparing the two spellings must not depend on which module supplied the id.
    """
    name = _type_name(type_id)
    return name[2:] if name.startswith("So") else name


class _ViewVolume:
    """Coin's ``SbViewVolume`` as far as picking needs it.

    ``projectPointToLine`` takes normalized viewport coordinates (``0..1``, **bottom-
    left** origin, exactly as Coin does) and returns the near and far world points of
    that line of sight.
    """

    __slots__ = ("_camera",)

    def __init__(self, camera):
        self._camera = camera

    def projectPointToLine(self, point):
        getter = getattr(point, "getValue", None)
        values = getter() if callable(getter) else tuple(point)
        if len(values) < 2:
            raise ValueError("projectPointToLine needs normalized (x, y)")
        ray = self._camera.ray(float(values[0]), float(values[1]))
        if ray is None:
            raise RuntimeError("the view camera has no invertible view-projection")
        near, far = ray
        return _Vec3(*near), _Vec3(*far)

    def __repr__(self):
        return "SbViewVolume"


class _CameraNode:
    """A snapshot of the shell's camera, shaped like a Coin ``SoCamera``.

    ``pivy.coin`` in FerroCAD's build has no camera classes, so the facade presents the
    published camera with the members a workbench actually reads: ``position``,
    ``orientation``, ``focalDistance``, ``height``, ``heightAngle``, ``aspectRatio``,
    ``getTypeId``/``isOfType`` and ``getViewVolume``. Ask the view for it again after
    the camera moves; each node is a snapshot.
    """

    __slots__ = ("_camera",)

    def __init__(self, camera):
        self._camera = camera

    @property
    def position(self):
        return _Field("position", _Vec3(*self._camera.position))

    @property
    def orientation(self):
        return _Field("orientation", _Quaternion(self._camera.orientation))

    @property
    def focalDistance(self):
        return _Field("focalDistance", self._camera.focal_distance)

    @property
    def height(self):
        return _Field("height", self._camera.height)

    @property
    def heightAngle(self):
        return _Field("heightAngle", self._camera.height_angle)

    @property
    def aspectRatio(self):
        return _Field("aspectRatio", self._camera.aspect_ratio)

    def getField(self, name):
        """The named field, or ``None`` — Coin returns ``None`` for an unknown name."""
        if name in ("position", "orientation", "focalDistance", "height", "heightAngle", "aspectRatio"):
            return getattr(self, name)
        return None

    def getViewVolume(self, aspect=None):
        """The view volume (``aspect`` is accepted for Coin compatibility)."""
        del aspect
        return _ViewVolume(self._camera)

    def getTypeId(self):
        """The camera type: ``SoPerspectiveCamera`` or ``SoOrthographicCamera``."""
        return _CameraType(
            "SoOrthographicCamera" if self._camera.is_orthographic else "SoPerspectiveCamera"
        )

    def isOfType(self, type_id):
        """Whether this camera is ``type_id`` (a Coin ``SoType``, by name)."""
        name = _type_key(type_id)
        if name == "Camera":
            return True
        return name == _type_key(self.getTypeId())

    def getSize(self):
        return tuple(self._camera.size)

    def __repr__(self):
        return f"SoCamera({_type_name(self.getTypeId())}, position={_Vec3(*self._camera.position)})"


class _EventCallback:
    """A Coin ``SoEventCallback``-shaped handle for ``addEventCallbackPivy``.

    The callback receives this object, so ``callback.getEvent()`` and
    ``callback.setHandled()`` are the Coin spellings a tool already uses. The event is
    only present while the shell is dispatching it: this facade never invents one.
    """

    __slots__ = ("_node", "_registration", "data", "handled")

    def __init__(self, node, registration, data=None):
        self._node = node
        self._registration = registration
        self.data = data
        self.handled = False

    def getEvent(self):
        if self._node is None:
            return None
        return self._node.getEvent()

    def getPickedPoint(self):
        if self._node is None:
            return None
        return self._node.getPickedPoint()

    def getAction(self):
        """Coin's action is not modelled; the shell delivers events, not actions."""
        return None

    def setHandled(self, *_args):
        """Mark the event consumed (the shell decides what a consumed event stops)."""
        self.handled = True

    def __repr__(self):
        return f"SoEventCallback({self._registration.name!r})"


class _PivyRegistration:
    """One ``addEventCallbackPivy`` registration.

    ``SoEventCallback`` has no ``removeEventCallback`` in FerroCAD's Coin facade, so a
    removed registration stays on the node as a wrapper that forwards nothing. The
    wrapper count is bounded by add/remove cycles, not by events.
    """

    __slots__ = ("view", "name", "callback", "active", "data", "handle")

    def __init__(self, view, name, callback, data=None):
        self.view = view
        self.name = name
        self.callback = callback
        self.data = data
        self.active = True
        self.handle = None

    def deactivate(self):
        self.active = False

    def dispatch(self, *_args):
        """Forward one event to the user callback (no-op once removed).

        The callback receives the :class:`_EventCallback` handle, not the Coin node:
        that is the object ``getEvent``/``setHandled`` are documented on, and the one
        the caller keeps for ``removeEventCallbackPivy``.
        """
        if not self.active:
            return
        self.callback(self.handle)

    def __repr__(self):
        return f"SoEventCallback({self.name!r}, active={self.active})"


class View3DInventor:
    """A document's 3D view, shaped like upstream's ``View3DInventor``.

    ``getSceneGraph`` is the real Coin surface: enough for a script to attach a node.
    The rest of the view protocol falls into two halves:

    * **the published view** — ``getCameraNode``, ``getViewDirection``,
      ``getPointOnScreen``, ``getSize``, ``getObjectsInfo`` and ``getObjectInfo`` read
      the camera and stage the shell **publishes** into ``ferrocad_stage``
      (``set_view_camera``/``set_view_stage``). They are not frozen defaults: a
      published camera answers each call, and a headless script or test publishes its
      own the same way. With nothing published ``getCameraNode`` is ``None`` and
      ``getViewDirection`` reports ``-Z``; a screen query then raises, because no
      camera means no honest projection.
    * **request bookkeeping** — ``fitAll``, ``redraw`` and ``updateGui`` record the
      request (there is no window here; the shell paints from its own state), and the
      event callbacks keep the Coin-shaped registrations a tool adds.
    """

    def __init__(self, document: "GuiDocument") -> None:
        self._document = document
        # FreeCADGui's view protocol: event name -> callbacks (the string spelling).
        self._event_callbacks: dict = {}
        # The Pivy spelling: Coin event type ids, wired to one `SoEventCallback` node.
        self._pivy: list = []
        self._pivy_node = None
        self._pivy_attached = False
        self._fits: int = 0
        self._redraws: int = 0
        self._updates: int = 0

    # -- the scene graph ----------------------------------------------------

    def getSceneGraph(self):
        """The document's scene root (``None`` when Coin is unavailable)."""
        return self._document._scene_graph()

    # -- the published view -------------------------------------------------

    def _document_name(self) -> str:
        """The name the pick dictionaries report as ``Document``."""
        name = getattr(self._document, "Name", None)
        if isinstance(name, str) and name:
            return name
        document = FreeCAD.ActiveDocument
        name = getattr(document, "Name", None)
        return name if isinstance(name, str) else ""

    def getCameraNode(self):
        """The published camera as a Coin-``SoCamera``-shaped snapshot, or ``None``.

        Ask again after the camera moves: each call returns a fresh snapshot of the
        camera the shell last published.
        """
        camera = _published_camera()
        if camera is None:
            return None
        return _CameraNode(camera)

    def getViewDirection(self):
        """The world direction the camera looks along.

        ``-Z`` when nothing has been published, which is the direction a fresh
        FerroCAD view looks along; a published camera is always authoritative.
        """
        camera = _published_camera()
        if camera is None:
            return FreeCAD.Vector(0.0, 0.0, -1.0)
        return FreeCAD.Vector(*camera.direction)

    def getPointOnScreen(self, point):
        """Project a world point to device pixels (bottom-left origin).

        ``None`` when the point is behind the eye (a perspective point on the ``w = 0``
        plane has no screen position). Raises ``RuntimeError`` when no camera has been
        published, because any answer would be invented.
        """
        camera = self._require_camera("getPointOnScreen")
        projected = camera.project(_triple(point))
        return None if projected is None else (projected[0], projected[1])

    def getSize(self):
        """The device-pixel viewport size, or ``(0, 0)`` when unknown."""
        camera = _published_camera()
        return tuple(camera.size) if camera is not None else (0, 0)

    def getObjectsInfo(self, pos):
        """Every sub-element under device pixel ``pos``, nearest first.

        Each entry is a plain dict shaped like upstream's:
        ``{'Document', 'Object', 'Component', 'x', 'y', 'z'}``. The list is a real
        ranking: hits are ordered by distance along the ray, one entry per
        ``(object, sub-element)`` — the occlusion stack a tool sorts its own
        candidates from.

        Two limits are deliberate. A hit reports **the stage the shell drew**, so a
        transient preview and the grid are never picked, exactly as a click sees it.
        And a ``SC_Support`` profile is not promoted here: that priority belongs to the
        document, and keeping the list ordered lets the caller apply it (SimpleCAD's
        ``pick`` does).
        """
        return [
            {
                "Document": self._document_name(),
                "Object": hit.object,
                "Component": hit.part or "",
                "x": hit.point[0],
                "y": hit.point[1],
                "z": hit.point[2],
            }
            for hit in self._pick_hits(pos)
            if hit.object
        ]

    def getObjectInfo(self, pos):
        """The nearest entry :meth:`getObjectsInfo` would return, or ``None``."""
        hits = self.getObjectsInfo(pos)
        return hits[0] if hits else None

    def _pick_hits(self, pos):
        """The ranked picks under ``pos`` from the published stage."""
        module = _stage_module()
        picker = getattr(module, "pick_at", None)
        if picker is None:
            return []
        try:
            x, y = int(pos[0]), int(pos[1])
        except (TypeError, IndexError, ValueError) as exc:
            raise ValueError("a pick position needs (x, y) device pixels") from exc
        return picker(x, y)

    def _require_camera(self, what: str):
        camera = _published_camera()
        if camera is None:
            raise RuntimeError(
                f"{what} needs a view camera; none has been published. The shell "
                "publishes it while it draws, and a script can publish one with "
                "ferrocad_stage.set_view_camera(...)"
            )
        return camera

    # -- redraw and fit -----------------------------------------------------

    def fitAll(self):
        """Accept a fit request (fitting is the shell's; nothing moves here)."""
        self._fits += 1
        return True

    def redraw(self):
        """Accept a redraw request (the shell repaints from its own state)."""
        self._redraws += 1
        return True

    def updateGui(self):
        """Request a repaint (upstream ``Gui.updateGui``).

        A preview change asks the view to show it; the request is recorded here and
        counted globally (``FreeCADGui._take_update_requests``) because a script
        cannot repaint the shell's window itself.
        """
        global _UPDATE_REQUESTS
        _UPDATE_REQUESTS += 1
        self._updates += 1
        return True

    # -- the string-keyed callback registry --------------------------------

    def addEventCallback(self, name, callback) -> None:
        """Register ``callback`` for the event ``name``.

        Coin is absent in a headless build, so the callbacks are kept here for the
        shell/tests to drain; no event is ever emitted by this view.
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

    # -- the Pivy spelling --------------------------------------------------

    def addEventCallbackPivy(self, event_type, callback, data=None):
        """``view.addEventCallbackPivy(SoEvent.getClassTypeId(), cb)``.

        This is the Coin spelling the controller uses. The registration is wired to an
        ``SoEventCallback`` node in the document's scene graph, so the shell's event
        dispatch reaches it exactly as it reaches a scene graph a workbench built; the
        callback receives an :class:`_EventCallback` handle with ``getEvent``,
        ``getPickedPoint`` and ``setHandled``. Returns that handle, which is what
        :meth:`removeEventCallbackPivy` takes back.

        Without Coin (or without ``SoEventCallback`` in the installed facade) the
        registration is kept on the view for the host to drive; it still returns a
        handle, and no event is invented.
        """
        if not callable(callback):
            raise TypeError("an event callback must be callable")
        name = _type_name(
            event_type.getClassTypeId()
            if callable(getattr(event_type, "getClassTypeId", None))
            else event_type
        )
        registration = _PivyRegistration(self, name, callback, data)
        node = self._callback_node()
        handle = _EventCallback(node, registration, data)
        registration.handle = handle
        if node is not None:
            try:
                node.addEventCallback(event_type, registration.dispatch)
            except Exception as exc:  # noqa: BLE001 - a foreign Coin facade
                _report_view_once("pivy.coin.SoEventCallback.addEventCallback", exc)
            else:
                self._attach_callback_node()
        self._pivy.append(registration)
        return handle

    def removeEventCallbackPivy(self, event_type, callback):
        """Undo :meth:`addEventCallbackPivy`; returns whether it was registered.

        ``callback`` is the handle that call returned (the user callback itself is
        accepted too). The ``SoEventCallback`` node stays in the scene graph: the Coin
        facade has no per-callback removal, so the removed registration forwards
        nothing and the node's wrapper count grows by one per add/remove cycle
        instead of per event.
        """
        name = _type_name(
            event_type.getClassTypeId()
            if callable(getattr(event_type, "getClassTypeId", None))
            else event_type
        )
        registration = getattr(callback, "_registration", None)
        if registration is None:
            registration = next(
                (
                    entry
                    for entry in self._pivy
                    if (entry.callback is callback or entry.callback == callback)
                    and entry.name == name
                ),
                None,
            )
        if registration is None or not registration.active:
            return False
        registration.deactivate()
        try:
            self._pivy.remove(registration)
        except ValueError:
            pass
        return True

    def _callback_node(self):
        """The view's single ``SoEventCallback`` node, created on first use."""
        if self._pivy_node is not None:
            return self._pivy_node
        coin = _coin_module()
        factory = getattr(coin, "SoEventCallback", None) if coin is not None else None
        if factory is None:
            return None
        try:
            self._pivy_node = factory()
        except Exception as exc:  # noqa: BLE001 - report, keep the registry
            _report_view_once("pivy.coin.SoEventCallback", exc)
            return None
        return self._pivy_node

    def _attach_callback_node(self) -> None:
        """Put the callback node in the scene graph and refresh the shell's copy."""
        if self._pivy_node is None:
            return
        root = self.getSceneGraph()
        if root is None:
            return
        if not self._pivy_attached:
            try:
                root.addChild(self._pivy_node)
            except Exception as exc:  # noqa: BLE001 - report, keep the registry
                _report_view_once("pivy.coin.SoEventCallback.addChild", exc)
                return
            self._pivy_attached = True
        # An overlay is a copy: re-register the root so the shell compiles the node.
        register = getattr(self._document, "_register_overlay", None)
        if callable(register):
            register()

    # -- the Snapper point session -----------------------------------------

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


#: Repaint requests made through ``FreeCADGui.updateGui`` since the host last took
#: them. The shell owns the window, so this is a request count, not a paint.
_UPDATE_REQUESTS: int = 0


def updateGui() -> None:
    """Request a repaint (upstream ``Gui.updateGui``).

    A command that changed a preview calls this so the view shows it. FerroCAD has no
    Python-visible window: the request is recorded on the active view and counted
    globally (``_take_update_requests``), and the shell's next paint shows the change
    from its own state. Nothing here can wake a window.
    """
    global _UPDATE_REQUESTS
    updater = getattr(getMainWindow().getActiveWindow(), "updateGui", None)
    if callable(updater):
        updater()
    else:
        _UPDATE_REQUESTS += 1


def _take_update_requests() -> int:
    """Consume and return the repaint requests made since the last call."""
    global _UPDATE_REQUESTS
    count, _UPDATE_REQUESTS = _UPDATE_REQUESTS, 0
    return count


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
    global _pending_tool
    command = _COMMANDS.get(name)
    if command is None:
        return {"ok": False, "error": f"unknown command {name!r}"}
    # A request from an earlier direct ``Activated`` call must not attach to this
    # command. Read the flag *after* the call, below.
    _pending_tool = None
    try:
        result = command.Activated()
    except Exception as exc:  # noqa: BLE001 - one broken command is not fatal
        return {"ok": False, "error": _error_text(exc)}
    payload = _payload(result)
    pending, _pending_tool = _pending_tool, None
    if pending is not None:
        payload.setdefault("tool", pending)
    return payload


def _request_tool(tool_id: str) -> None:
    """Ask the shell to start ``tool_id`` after ``Activated`` returns.

    Returns ``None`` so a command's upstream-shaped ``Activated`` can end with
    ``return FreeCADGui._request_tool(...)``. The id is forwarded verbatim; the
    shell starts a Python-backed bridge for a ``python:<name>`` id.
    """
    global _pending_tool
    _pending_tool = str(tool_id)
    return None


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
    The shell-facing keys are ``title``, ``fields``, ``accept``, ``cancel``,
    ``presentation`` and ``preview``; ``status``, ``value``, ``view``, ``tool``,
    ``select`` and ``gizmo`` stay top-level; everything else is ignored until a
    later slice gives it meaning.
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
    if "gizmo" in result:
        payload["gizmo"] = result["gizmo"]
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
    global _pending_tool
    _DIALOGS.clear()
    _pending_tool = None


class Preview:
    """Transient, unpickable shapes drawn over the viewport (S4.6).

    SimpleCAD draws its construction previews as imperative Coin subtrees. FerroCAD's
    equivalent is a **transient `ShapeItem`**: the shell draws it beside the document
    stage and the picker ignores it (an item is forced non-selectable when it is
    published), so a preview never answers a click and never appears in
    :meth:`View3DInventor.getObjectsInfo`.

    Building items is the caller's job — this is the presentation channel, not a
    shape factory. ``add`` takes a ready ``ferrocad_stage.ShapeItem`` (the declarative
    route a ``ViewProvider`` uses); ``add_mesh`` is the convenience for a tessellation
    a tool already has.

    Publishing is explicit, like Coin's own ``replaceChild``: add/remove/clear edit
    the local set and :meth:`update` hands it to the shell. A headless caller sees the
    same set through ``ferrocad_stage.preview_items()``.
    """

    #: The first item id a preview claims. Document entities are small counters, and
    #: the push/pull stem uses 2 100 000, so this range is free.
    BASE_ID = 3_000_000

    def __init__(self, view=None) -> None:
        self._view = view
        self._items: dict = {}
        self._next = 0
        self._serial = 0

    def _item_module(self):
        module = _stage_module()
        if module is None or not hasattr(module, "ShapeItem"):
            raise RuntimeError(
                "FreeCADGui.Preview needs the `ferrocad_stage` extension; build it "
                "with `cargo build -p ferrocad_stage_py`"
            )
        return module

    def add(self, item) -> int:
        """Draw ``item`` (a ``ferrocad_stage.ShapeItem``) until it is removed."""
        if not hasattr(item, "set_shape") or not hasattr(item, "entity_id"):
            raise TypeError("Preview.add needs a ferrocad_stage.ShapeItem")
        handle = self._next
        self._next += 1
        self._items[handle] = item
        return handle

    def add_mesh(
        self,
        positions,
        triangles=None,
        edges=None,
        color=None,
        shaded=True,
    ) -> int:
        """Draw a tessellation (``positions`` plus optional triangle/edge indices).

        ``shaded`` picks ``ShadedWithEdges`` (a solid preview) over ``Wireframe`` (a
        construction guide); ``color`` is an RGBA tuple applied to the surface.
        """
        module = self._item_module()
        self._serial += 1
        item = module.ShapeItem(self.BASE_ID + self._serial)
        item.set_shape(positions, None, triangles, edges)
        item.draw_mode = (
            module.DrawMode.ShadedWithEdges if shaded else module.DrawMode.Wireframe
        )
        if color is not None:
            rgba = tuple(float(value) for value in color)
            item.surface_color = rgba if len(rgba) == 4 else rgba + (1.0,)
        item.selectable = False
        return self.add(item)

    def remove(self, handle: int) -> bool:
        """Stop drawing the item ``handle``; returns whether it was present."""
        return self._items.pop(handle, None) is not None

    def clear(self) -> None:
        """Drop every preview item in this set."""
        self._items.clear()

    def items(self) -> list:
        """The items in publication order (a copy; the set is what ``update`` sends)."""
        return list(self._items.values())

    def update(self) -> int:
        """Publish the set and ask for a repaint; returns the new generation."""
        module = _stage_module()
        if module is None or not hasattr(module, "set_preview_items"):
            raise RuntimeError(
                "FreeCADGui.Preview.update needs the `ferrocad_stage` extension"
            )
        generation = module.set_preview_items(self.items())
        view = self._view
        if view is None:
            document = activeDocument()
            view = getattr(document, "ActiveView", None) if document is not None else None
        if view is not None:
            view.updateGui()
        return generation

    def __len__(self) -> int:
        return len(self._items)

    def __repr__(self) -> str:
        return f"Preview({len(self)} items)"


# ---------------------------------------------------------------------------
# Selection (the shell owns it; writes are queued back to it)
# ---------------------------------------------------------------------------


class SelectionObject:
    """One selected object, shaped like upstream's ``Gui.SelectionObject``.

    ``Object`` is the document object itself, ``SubElementNames`` the picked
    sub-elements (``["Face6"]``; empty for the whole object), ``PickedPoints`` the
    world points the picks landed on (as far as the source knew them), and
    ``ObjectName`` the object's internal name.

    It unpacks as ``(Object, SubElementNames)``, so code written against the older
    tuple snapshot keeps working.
    """

    # Declared for the generated reference; `__init__` always sets the instance
    # attributes (`api_docs` documents what a class declares, not what it assigns).
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
    def HasSubObjects(self) -> bool:
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
        return f"SelectionObject({self.ObjectName!r}, {self.SubElementNames!r})"


class _Selection:
    """What the shell has selected, in upstream's shape.

    The shell owns the selection and pushes it (``_set``) after every viewport
    change, so ``getSelection``/``getSelectionEx`` always read the live document. The
    writable half (``addSelection``/``removeSelection``/``clearSelection``/
    ``setPreselection``) applies to this snapshot **and** queues the operation for the
    shell (``_take_requests``): Python cannot command the viewport synchronously, and
    silently pretending the viewport changed would be a lie.

    An entry is ``(name, [sub-elements], [picked points])``; the pushed snapshot
    carries no picked points, so a shell-made selection reports an empty
    ``PickedPoints`` unless the writer supplied one.
    """

    def __init__(self) -> None:
        self._entries: list = []
        self._preselection = None
        self._requests: list = []

    # -- reads --------------------------------------------------------------

    def _document(self, doc=None):
        if doc is None:
            return FreeCAD.ActiveDocument
        if isinstance(doc, str):
            return FreeCAD.getDocument(doc)
        return doc

    def _objects(self, doc=None, object_name=None) -> list:
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

    def getSelection(self, doc=None) -> list:
        """The selected document objects (upstream ``Gui.Selection``)."""
        return [item.Object for item in self._objects(doc)]

    def getSelectionEx(self, object_name=None, doc=None) -> list:
        """The selection as :class:`SelectionObject` items.

        An item's ``.Object``/``.SubElementNames``/``.PickedPoints`` are the upstream
        members, and unpacking one as ``(object, elements)`` still works.
        """
        return self._objects(doc, object_name)

    def isSelected(self, obj, subname=None) -> bool:
        """Whether ``obj`` (or an object name) is in the selection.

        With ``subname``, only a selection of that sub-element (or of the whole
        object) answers ``True``.
        """
        name = obj if isinstance(obj, str) else getattr(obj, "Name", None)
        for candidate, elements, _ in self._entries:
            if candidate != name:
                continue
            if subname is None or not elements or subname in elements:
                return True
        return False

    # -- writes (applied here, queued for the shell) ------------------------

    def addSelection(self, obj, subname=None, point=None, doc=None) -> bool:
        """Add ``obj`` (with optional sub-elements) to the selection.

        ``subname`` is a name or a list of names. ``point`` is the world point the
        pick landed on (a Vector, or a list of them, one per sub-element). An object
        the document does not have is ignored and reported as ``False``: the facade
        never invents an object.
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

    def removeSelection(self, obj, subname=None, doc=None) -> bool:
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

    def clearSelection(self, doc=None) -> None:
        """Clear the selection (``doc`` limits it to one document's objects)."""
        if doc is None:
            self._entries = []
        else:
            document = self._document(doc)
            names = {
                obj.Name
                for obj in (getattr(document, "Objects", None) or [])
            }
            self._entries = [entry for entry in self._entries if entry[0] not in names]
        self._requests.append({"op": "clear", "object": "", "elements": [], "points": []})

    def setPreselection(self, obj, subname=None, point=None) -> None:
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
        document = FreeCAD.ActiveDocument
        obj = document.getObject(name) if document is not None and name else None
        if obj is None:
            return None
        return SelectionObject(obj, elements, points)

    # -- the shell bridge ---------------------------------------------------

    def _set(self, pairs) -> None:
        """Replace the snapshot (called by the shell bridge).

        Accepts ``(name, elements)`` pairs or ``(name, elements, points)`` triples, so
        the existing shell push keeps working and a future one can carry the picked
        points without a second entry point. Writes made here are **not** queued: the
        snapshot is the shell's answer, not a request back to it.
        """
        entries = []
        for entry in pairs:
            name, elements = entry[0], entry[1]
            points = entry[2] if len(entry) > 2 else None
            entries.append((str(name), _names(elements), _points(points)))
        self._entries = entries
        self._preselection = None

    def _take_requests(self) -> list:
        """Consume the queued writes for the shell to apply to the viewport."""
        requests, self._requests = self._requests, []
        return requests


def _names(subname) -> list:
    """A sub-element selection as a list of names (``None`` → the whole object)."""
    if subname is None:
        return []
    if isinstance(subname, str):
        return [subname] if subname else []
    return [str(name) for name in subname if str(name)]


def _points(point) -> list:
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
