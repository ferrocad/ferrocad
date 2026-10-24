"""The native Draft control surface: ``Snapper``, ``draftToolBar`` and the snap bar.

This is the Q4 slice of the Qt-reduction plan. The camera-free snapping maths
lives in the Rust extension ``ferrocad_stage`` (upstream Draft's ``Snapper`` was
a screen-space calculator); the Python layer here is the **upstream-shaped**
adapter that Draft tools and macros import as ``FreeCADGui.Snapper``.

What is native
--------------

* six snap modes -- ``Grid``, ``Endpoint``, ``Midpoint``, ``Center``, ``Origin``
  and ``Vertex`` -- backed by ``ferrocad_stage.SnapContext``;
* the workplane/grid frame, including an in-plane grid rotation
  (:meth:`Snapper.setAngle`);
* point sessions (:meth:`Snapper.getPoint`) driven headlessly by
  :func:`pick` / :func:`escape`, with the same one-pending-request rule the
  interactive tools rely on;
* a stored point-constraint provider and axis constraint
  (:meth:`Snapper.constrain`), which ``draftToolBar.validatePoint`` consults;
* the six checkable ``Draft_Snap_*`` commands.

What is deliberately absent
----------------------------

The Draft-only snaps (``Extension``, ``Parallel``, ``Perpendicular``,
``Intersection``, ``Angle``, ``Special``, ``Dimensions``, ``Ortho`` and
``Lock``) need an edge/face extension solver and a live cursor; enabling one
raises ``NotImplementedError`` instead of pretending. Coin trackers are absent
too, so :meth:`Snapper.restack` and :meth:`Snapper.cycleSnapObject` raise, while
:meth:`Snapper.setTrackers` does the one thing it can natively: refresh and
return the current :class:`SnapContext`.

The module never imports ``ferrocad_stage`` at import time. ``import FreeCADGui``
must keep working in a build where the extension is missing; the extension is
loaded on the first Snapper query and a clear ``ImportError`` names the fix.
"""

from __future__ import annotations

import math
import sys

import FreeCADGui

__all__ = [
    "Snapper",
    "draftToolBar",
    "HintManager",
    "InputHint",
    "UserInput",
    "pick",
    "escape",
    "finish",
    "supported_modes",
    "unsupported_modes",
    "SNAP_TOOLBAR",
]

#: The snap modes with a native ``SnapKind`` behind them, in toolbar order.
_SUPPORTED_MODES = ("Grid", "Endpoint", "Midpoint", "Center", "Origin", "Vertex")

#: Draft modes that need a live cursor / extension solver, in upstream order.
_UNSUPPORTED_MODES = (
    "Extension",
    "Parallel",
    "Perpendicular",
    "Intersection",
    "Angle",
    "Special",
    "Dimensions",
    "Ortho",
    "Lock",
)

#: The global snap switch. The header's ``Snap`` control runs it on the label and
#: the six modes declare it as their ``FerroCAD_Parent``.
_SNAP_LOCK_ID = "Draft_Snap_Lock"

#: ``mode -> command id`` for the checkable snap bar, in toolbar order.
_COMMAND_IDS = {
    "Grid": "Draft_Snap_Grid",
    "Endpoint": "Draft_Snap_Endpoint",
    "Midpoint": "Draft_Snap_Midpoint",
    "Center": "Draft_Snap_Center",
    "Origin": "Draft_Snap_Origin",
    "Vertex": "Draft_Snap_Vertex",
}

#: The checkable snap-bar command ids (what ``get_snap_toolbar`` returns).
SNAP_TOOLBAR = tuple(_COMMAND_IDS[mode] for mode in _SUPPORTED_MODES)

#: ``mode -> SnapKind name``. ``Endpoint`` and ``Vertex`` are both topological
#: vertices in the native snap API; the two Draft modes remain distinct names in
#: ``mask`` because the toolbar exposes both upstream.
_MODE_KINDS = {
    "Endpoint": "Vertex",
    "Midpoint": "EdgeMid",
    "Center": "Centre",
    "Origin": "Origin",
    "Vertex": "Vertex",
}

#: The extension module, imported on first use (``import FreeCADGui`` stays safe).
_STAGE = None
_STAGE_ERROR = None


def _stage_module():
    """The ``ferrocad_stage`` extension, imported lazily and cached."""
    global _STAGE, _STAGE_ERROR
    if _STAGE is not None:
        return _STAGE
    if _STAGE_ERROR is not None:
        raise ImportError(_stage_error_message()) from _STAGE_ERROR
    try:
        import ferrocad_stage as stage
    except ImportError:
        try:
            # ``install()`` also registers ``ferrocad.stage``; accept either
            # spelling so a host that installed the module is usable as-is.
            from ferrocad import stage as stage  # type: ignore[no-redef]
        except Exception as exc:  # noqa: BLE001 - report the actionable error
            _STAGE_ERROR = exc
            raise ImportError(_stage_error_message()) from exc
    _STAGE = stage
    return stage


def _stage_error_message() -> str:
    return (
        "FreeCADGui.Snapper needs the `ferrocad_stage` extension module. "
        "Build it with `cargo build -p ferrocad_stage_py --features "
        "ferrocad_stage_py/extension-module` and make the resulting library "
        "importable as `ferrocad_stage` (for example by copying "
        "`libferrocad_stage_py.so` to `ferrocad_stage.abi3.so` in the "
        "`python/` directory)."
    )


def _triple(value):
    """Normalize a point-like value to ``(x, y, z)`` floats.

    Accepts a 3-sequence or any object with ``x``/``y``/``z`` (``FreeCAD.Vector``
    and ``Snap.point`` both qualify).
    """
    if hasattr(value, "x") and hasattr(value, "y") and hasattr(value, "z"):
        return (float(value.x), float(value.y), float(value.z))
    try:
        x, y, z = value
    except (TypeError, ValueError) as exc:
        raise TypeError(
            "a point must be an (x, y, z) sequence or have x/y/z attributes, "
            f"got {type(value).__name__}"
        ) from exc
    return (float(x), float(y), float(z))


def _dot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


def _cross(a, b):
    return (
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    )


def _unit(value):
    x, y, z = _triple(value)
    length = math.sqrt(x * x + y * y + z * z)
    if length <= 1e-15:
        raise ValueError("a constraint axis must be a non-zero vector")
    return (x / length, y / length, z / length)


def supported_modes():
    """The snap modes with a native implementation, in toolbar order."""
    return list(_SUPPORTED_MODES)


def unsupported_modes():
    """The Draft-only snap modes, in upstream order.

    Enabling any of these raises ``NotImplementedError``; the list is exposed so
    a menu can grey them out instead of discovering that one by one.
    """
    return list(_UNSUPPORTED_MODES)


class _PointRequest:
    """One pending ``getPoint`` callback and the arguments it receives.

    ``on_cancel`` runs when :meth:`Snapper.off` ends the request (Escape), and
    ``on_finish`` when :func:`finish` ends it (Enter). Both are optional: a
    caller that only wants a point leaves them unset and simply never hears
    about the end of the session.
    """

    __slots__ = ("callback", "args", "on_cancel", "on_finish")

    def __init__(self, callback, args, on_cancel=None, on_finish=None):
        self.callback = callback
        self.args = tuple(args)
        self.on_cancel = on_cancel
        self.on_finish = on_finish

    def invoke(self, point):
        return self.callback(point, *self.args)


class _Snapper:
    """The module-level snapper singleton (``FreeCADGui.Snapper``).

    ``mask`` holds the active Draft mode names; the grid/workplane state and the
    ``affinity`` feature tolerance are the inputs to every :meth:`snap`. The
    object keeps no view state: the active ``SnapContext`` is rebuilt from the
    workplane, grid flag and rotation, and a point session is one pending
    callback.
    """

    def __init__(self):
        #: The tool-bar adapter (``draftToolBar``); assigned when it is built.
        self.ui = None
        #: The active Draft snap mode names.
        self._mask = list(_SUPPORTED_MODES)
        #: Select mode suppresses the interactive cursor upstream; kept as state.
        self.selectMode = False
        #: Feature-snap tolerance in world units. 1.0 matches one default cell.
        self.affinity = 1.0
        #: Whether the workplane grid is visible/usable.
        self.grid = True
        #: The most recent :class:`ferrocad_stage.Snap`, or ``None``.
        self.snapInfo = None
        #: Hold points added by :meth:`addHoldPoint` (world coordinates).
        self.holdPoints = []
        #: The active task-panel point constraint provider, if any.
        self.pointConstraintProvider = None
        #: The base point and axis of the current ``constrain`` state.
        self.basepoint = None
        self.constraintAxis = None
        #: Whether a point session is live (upstream spells it ``running``).
        self.running = False

        self._enabled = True
        self._pending = None
        self._minor = 1.0
        self._major = 10.0
        self._base_workplane = None
        self._angle = 0.0
        #: The last hint text mirrored by ``HintManager`` (shell/status channel).
        self.hint = ""

    # -- modes ------------------------------------------------------------

    @property
    def mask(self):
        """The active mode names (a live list, in activation order)."""
        return self._mask

    @mask.setter
    def mask(self, value):
        if value is None:
            self._mask = []
            return
        if isinstance(value, str):
            names = [value]
        else:
            names = list(value)
        modes = []
        for name in names:
            mode = self._enabled_mode_name(name)
            if mode not in modes:
                modes.append(mode)
        self._mask = modes

    @staticmethod
    def _enabled_mode_name(mode):
        """Validate a mode name for an **enabling** operation."""
        if not isinstance(mode, str):
            raise TypeError(f"a snap mode must be a string, got {type(mode).__name__}")
        if mode in _UNSUPPORTED_MODES:
            raise NotImplementedError(
                f"Snapper: the {mode!r} snap mode needs a live cursor or an "
                "edge/face extension solver and is not implemented natively; "
                "supported modes are "
                + ", ".join(_SUPPORTED_MODES)
            )
        if mode not in _SUPPORTED_MODES:
            raise ValueError(
                f"Snapper: unknown snap mode {mode!r}; supported modes are "
                + ", ".join(_SUPPORTED_MODES)
            )
        return mode

    def mode_enabled(self, mode):
        """Whether ``mode`` is in ``mask`` (an unsupported mode is never on)."""
        return mode in self._mask

    def isEnabled(self, snap=None):
        """Whether snapping is globally enabled, or whether ``snap`` is on.

        Upstream calls ``isEnabled("Endpoint")``; ``isEnabled()`` is the global
        check the snap-bar commands use for ``IsActive``.
        """
        if snap is None:
            return bool(self._enabled)
        if snap in _UNSUPPORTED_MODES or snap not in _SUPPORTED_MODES:
            return False
        return snap in self._mask

    def set_enabled(self, enabled=True):
        """Enable or disable snapping globally; returns the new state.

        The per-mode ``mask`` is left untouched, so switching snapping back on
        restores exactly the filters the user had. This is the switch behind the
        ``Snap`` control in the viewport header (and ``Draft_Snap_Lock``).
        """
        self._enabled = bool(enabled)
        return self._enabled

    def toggle_enabled(self):
        """Flip the global snap switch; returns the new state."""
        return self.set_enabled(not self._enabled)

    #: Upstream-style camel-case aliases.
    setEnabled = set_enabled
    toggleEnabled = toggle_enabled

    def toggle_snap(self, snap, set_to=None):
        """Toggle a mode in ``mask`` (or set it explicitly); returns its state.

        Enabling a Draft-only mode raises ``NotImplementedError`` naming it.
        A no-op disable is accepted so a menu that clears every mode does not
        fail on one that was never implemented.
        """
        if snap in _UNSUPPORTED_MODES:
            if set_to is False:
                return False
            raise NotImplementedError(
                f"Snapper: the {snap!r} snap mode needs a live cursor or an "
                "edge/face extension solver and is not implemented natively"
            )
        self._enabled_mode_name(snap)
        if set_to is None:
            enabled = snap not in self._mask
        else:
            enabled = bool(set_to)
        if enabled:
            if snap not in self._mask:
                self._mask.append(snap)
        else:
            if snap in self._mask:
                self._mask.remove(snap)
        return enabled

    #: Upstream spells the same operation ``toggle_snap``.
    toggle = toggle_snap

    def set_mode(self, snap, enabled=True):
        """Explicitly set ``snap`` on or off; returns the resulting state."""
        return self.toggle_snap(snap, bool(enabled))

    #: ``setMode`` is the camel-case spelling some macros use.
    setMode = set_mode

    def enable(self, snap):
        return self.set_mode(snap, True)

    def disable(self, snap):
        return self.set_mode(snap, False)

    def active_snaps(self):
        """The enabled mode names."""
        return list(self.mask)

    @staticmethod
    def supported_modes():
        """The modes with a native implementation, in toolbar order."""
        return supported_modes()

    @staticmethod
    def unsupported_modes():
        """The Draft-only modes that raise when enabled, in upstream order."""
        return unsupported_modes()

    # -- workplane, grid and context --------------------------------------

    @property
    def workplane(self):
        """The effective workplane, with :meth:`setAngle` applied."""
        base = self._base_workplane
        if base is None:
            base = _stage_module().Workplane.world()
            self._base_workplane = base
        if not self._angle:
            return base
        stage = _stage_module()
        cos = math.cos(self._angle)
        sin = math.sin(self._angle)
        u = base.u
        v = base.v
        ru = (
            u[0] * cos + v[0] * sin,
            u[1] * cos + v[1] * sin,
            u[2] * cos + v[2] * sin,
        )
        rv = (
            -u[0] * sin + v[0] * cos,
            -u[1] * sin + v[1] * cos,
            -u[2] * sin + v[2] * cos,
        )
        return stage.Workplane(base.origin, ru, rv, base.kind)

    def setWorkplane(self, workplane):
        """Adopt ``workplane`` (a ``ferrocad_stage.Workplane``) and clear rotation."""
        if not all(hasattr(workplane, name) for name in ("origin", "u", "v")):
            raise TypeError("setWorkplane expects a ferrocad_stage.Workplane")
        self._base_workplane = workplane
        self._angle = 0.0
        return self.workplane

    #: Draft spells the same operation with an underscore too.
    set_workplane = setWorkplane

    @property
    def angle(self):
        """The grid rotation in degrees (the native unit is radians)."""
        return math.degrees(self._angle)

    @property
    def context(self):
        """The current ``SnapContext`` for the workplane and grid state."""
        stage = _stage_module()
        grid_on = bool(self.grid) and "Grid" in self._mask
        return stage.SnapContext(
            self.workplane,
            self._minor if grid_on else 0.0,
            self._major if grid_on else 0.0,
        )

    @context.setter
    def context(self, value):
        if not all(hasattr(value, name) for name in ("workplane", "minor", "major")):
            raise TypeError("context expects a ferrocad_stage.SnapContext")
        self._base_workplane = value.workplane
        self._angle = 0.0
        self._minor = float(value.minor)
        self._major = float(value.major)
        self.grid = self._minor > 0.0

    #: ``snap_context`` is an explicit alias for readers of the task spec.
    @property
    def snap_context(self):
        return self.context

    def setGrid(self, enabled=True):
        """Show or hide (and enable or disable) the workplane grid."""
        self.grid = bool(enabled)
        return self.grid

    def toggleGrid(self):
        """Flip the grid flag; returns the new state."""
        self.grid = not self.grid
        return self.grid

    def setAngle(self, angle):
        """Rotate the snap grid about the workplane normal.

        ``angle`` is interpreted as **degrees** unless its magnitude is at most
        ``2*pi``, in which case it is already radians. This accepts both the
        CAD-facing ``setAngle(90)`` and the maths-facing
        ``setAngle(math.pi / 2)``. ``None`` (or zero) clears the rotation.
        """
        if angle is None:
            self._angle = 0.0
            return self.workplane
        value = float(angle)
        if abs(value) > 2.0 * math.pi:
            value = math.radians(value)
        self._angle = value
        return self.workplane

    # -- snapping ---------------------------------------------------------

    def snap(self, point, candidates=None):
        """Snap a world ``point`` and return the resolved ``(x, y, z)``.

        ``candidates`` are ``ferrocad_stage.SnapCandidate`` values supplied by
        the caller (Draft's object under the cursor). Only the kinds enabled by
        ``mask`` are considered; the result is remembered in ``snapInfo``.
        ``affinity`` is the world-space tolerance for a feature candidate.
        """
        stage = _stage_module()
        world = _triple(point)
        allowed = self._allowed_kinds(stage)
        if candidates is None:
            candidates = ()
        elif hasattr(candidates, "kind"):
            # A single SnapCandidate is a convenient (and common) shorthand.
            candidates = (candidates,)
        filtered = []
        for candidate in candidates:
            kind = getattr(candidate, "kind", None)
            if kind is None:
                raise TypeError(
                    "snap candidates must be ferrocad_stage.SnapCandidate values"
                )
            if kind in allowed:
                filtered.append(candidate)
        if self.mode_enabled("Vertex") or self.mode_enabled("Endpoint"):
            filtered.extend(
                stage.SnapCandidate(point, stage.SnapKind.Vertex)
                for point in self.holdPoints
            )

        tolerance = 0.0 if self.affinity is None else float(self.affinity)
        result = self.context.snap_point(world, filtered, tolerance)
        self.snapInfo = result
        return result.point

    def _allowed_kinds(self, stage):
        # ``SnapKind`` is unhashable (PyO3 generates ``__eq__`` without
        # ``__hash__``), so this is a list and membership uses equality.
        kinds = []
        for mode, kind_name in _MODE_KINDS.items():
            if mode in self._mask:
                kind = getattr(stage.SnapKind, kind_name)
                if kind not in kinds:
                    kinds.append(kind)
        return kinds

    # -- sessions ---------------------------------------------------------

    def getPoint(self, callback, *args, on_cancel=None, on_finish=None):
        """Start a point request and return immediately.

        ``callback(point, *args)`` runs once when :func:`pick` resolves a point.
        A second request replaces the pending one and reports the replacement;
        it never stacks, matching upstream's one-session snap interaction.

        ``on_cancel`` runs when the request is ended by :meth:`off` (Escape) and
        ``on_finish`` when :func:`finish` ends it (Enter). A tool uses them to
        tear down its own session; neither is required for a plain point pick.
        """
        if not callable(callback):
            raise TypeError("Snapper.getPoint needs a callable callback")
        if on_cancel is not None and not callable(on_cancel):
            raise TypeError("Snapper.getPoint on_cancel must be callable")
        if on_finish is not None and not callable(on_finish):
            raise TypeError("Snapper.getPoint on_finish must be callable")
        if self._pending is not None:
            print(
                "FreeCADGui.Snapper: a point request was already pending; "
                "replacing it with the new request",
                file=sys.stderr,
            )
        self._pending = _PointRequest(callback, args, on_cancel, on_finish)
        self.running = True
        return None

    def _take_pending(self):
        pending, self._pending = self._pending, None
        self.running = False
        return pending

    def _drop_pending(self):
        return self._take_pending() is not None

    def off(self):
        """End the current point request without invoking its callback.

        The workplane/mask stay as they are -- ending a pick must not silently
        turn the user's snap modes off -- while the constraint and select mode,
        which belong to the interrupted interaction, are reset. A request that
        registered ``on_cancel`` hears about the cancellation here, so the tool
        that started it can tear down its own session.
        """
        pending = self._take_pending()
        self.selectMode = False
        self.unconstrain()
        self.snapInfo = None
        if pending is not None and pending.on_cancel is not None:
            return pending.on_cancel()
        return None

    def setSelectMode(self, enabled):
        """Set the select-mode flag (upstream hides snapping while selecting)."""
        self.selectMode = bool(enabled)

    # -- constraints ------------------------------------------------------

    def _axis_vector(self, axis):
        if isinstance(axis, str):
            key = axis.lower()
            workplane = self.workplane
            if key == "x":
                return _triple(workplane.u)
            if key == "y":
                return _triple(workplane.v)
            if key == "z":
                return _triple(workplane.n)
            raise ValueError(
                "a constraint axis must be 'x', 'y', 'z' or a vector, "
                f"got {axis!r}"
            )
        return _unit(axis)

    def _closest_axis(self, delta):
        workplane = self.workplane
        axes = (_triple(workplane.u), _triple(workplane.v), _triple(workplane.n))
        return max(axes, key=lambda axis: abs(_dot(delta, axis)))

    def constrain(self, point, basepoint=None, axis=None):
        """Project ``point`` onto the constraint line through ``basepoint``.

        ``axis`` may be ``"x"``/``"y"``/``"z"`` (workplane ``u``/``v``/``n``) or
        a vector; when it is omitted the closest of the three workplane axes to
        the segment is used. The base point and axis become the current
        constraint, which ``draftToolBar.validatePoint`` applies to later points.
        """
        world = _triple(point)
        base = self.basepoint if basepoint is None else _triple(basepoint)
        if base is None:
            base = world
        if axis is None:
            axis_vector = self.constraintAxis
            if axis_vector is None:
                delta = (
                    world[0] - base[0],
                    world[1] - base[1],
                    world[2] - base[2],
                )
                axis_vector = self._closest_axis(delta)
        else:
            axis_vector = self._axis_vector(axis)
        axis_vector = _unit(axis_vector)
        delta = (world[0] - base[0], world[1] - base[1], world[2] - base[2])
        length = _dot(delta, axis_vector)
        constrained = (
            base[0] + axis_vector[0] * length,
            base[1] + axis_vector[1] * length,
            base[2] + axis_vector[2] * length,
        )
        self.basepoint = base
        self.constraintAxis = axis_vector
        return constrained

    def unconstrain(self):
        """Drop the base point and constraint axis."""
        self.basepoint = None
        self.constraintAxis = None

    def setPointConstraintProvider(self, provider):
        """Set the task panel that constrains snapped points."""
        self.pointConstraintProvider = provider

    def clearPointConstraintProvider(self, provider=None):
        """Clear the provider, or only when it is ``provider``."""
        if provider is None or self.pointConstraintProvider is provider:
            self.pointConstraintProvider = None

    # -- compatibility surface -------------------------------------------

    def setTrackers(self, update_grid=True):
        """Refresh and return the current ``SnapContext``.

        FerroCAD draws no Coin trackers, so there is nothing to (re)create. The
        native equivalent of upstream's call is rebuilding the context from the
        workplane/grid/rotation state; returning it makes the refresh visible.
        """
        if update_grid:
            # Re-read the grid flag so a caller can use setTrackers as its
            # "the view changed" refresh hook.
            self.grid = bool(self.grid)
        return self.context

    def restack(self):
        """Raise: there is no Coin tracker to lower in a headless build."""
        raise NotImplementedError(
            "Snapper.restack(): FerroCAD has no Coin snap tracker to restack"
        )

    def addHoldPoint(self, point=None):
        """Add ``point`` (or the last snapped point) to the hold-snap list."""
        if point is None:
            if self.snapInfo is None:
                raise ValueError(
                    "Snapper.addHoldPoint needs a point or a previous snap"
                )
            point = self.snapInfo.point
        world = _triple(point)
        if world not in self.holdPoints:
            self.holdPoints.append(world)
        return True

    def show(self):
        """Show the workplane grid (the native half of upstream ``show``)."""
        return self.setGrid(True)

    def hide(self):
        """Hide the workplane grid (the native half of upstream ``hide``)."""
        return self.setGrid(False)

    def cycleSnapObject(self, *_args, **_kwargs):
        """Raise: cycling objects needs a screen-space cursor under the mouse."""
        raise NotImplementedError(
            "Snapper.cycleSnapObject(): FerroCAD has no hovering cursor to "
            "cycle snap objects through; pass explicit SnapCandidates to snap()"
        )

    def get_snap_toolbar(self):
        """The checkable snap-bar command ids, in toolbar order."""
        return list(SNAP_TOOLBAR)

    # -- test/session helpers --------------------------------------------

    def reset(self):
        """Restore the defaults (used by tests and a future workbench reset)."""
        self._mask = list(_SUPPORTED_MODES)
        self.selectMode = False
        self.affinity = 1.0
        self.grid = True
        self.snapInfo = None
        self.holdPoints = []
        self.pointConstraintProvider = None
        self.constrain_reset()
        self._enabled = True
        self._pending = None
        self.running = False
        self._minor = 1.0
        self._major = 10.0
        self._base_workplane = None
        self._angle = 0.0
        self.hint = ""

    def constrain_reset(self):
        """Alias for :meth:`unconstrain`, kept next to :meth:`reset`."""
        self.unconstrain()


#: The module-level singleton, exposed as ``FreeCADGui.Snapper``.
Snapper = _Snapper()


def pick(point):
    """Resolve ``point`` through the Snapper and invoke the pending callback once.

    This is the headless driver seam: a shell (or a test) supplies the world
    point a click would have produced. The callback is removed *before* it runs,
    so it cannot re-enter this request. Returns the snapped ``(x, y, z)``.
    """
    pending = Snapper._take_pending()
    snapped = Snapper.snap(point)
    if pending is not None:
        pending.invoke(snapped)
    return snapped


def escape():
    """Drop the pending point request without invoking it (Escape), then end snapping."""
    had_pending = Snapper._pending is not None
    Snapper.off()
    return had_pending


def finish():
    """End the pending request as *accepted* (Enter), without picking a point.

    A tool that collects points registers ``on_finish`` with
    :meth:`Snapper.getPoint`; this calls it so the tool can commit what it has
    and close its session. Returns whatever the handler returned, or ``False``
    when no request or no handler is pending.
    """
    pending = Snapper._take_pending()
    if pending is None or pending.on_finish is None:
        return False
    return pending.on_finish()


# ---------------------------------------------------------------------------
# The Draft task-panel adapter
# ---------------------------------------------------------------------------


class _DraftToolBar:
    """``FreeCADGui.draftToolBar``: the headless Draft task-panel adapter.

    Only the state the upstream commands actually read and write is live here.
    ``Activated``/``Deactivated``/``offUi`` manage ``sourceCmd`` and
    ``isTaskOn``; ``escape`` ends the Snapper session and finishes the source
    command; ``validatePoint`` applies the current constraint. The remaining
    widgets (``tray``, ``taskUi``, ``textValue``, ``wplabel``) are placeholders
    for the Qt task panel, which FerroCAD does not build.
    """

    def __init__(self):
        #: The panel is its own UI adapter (upstream ``self.ui``).
        self.ui = self
        #: Whether the mouse drives the task panel (upstream state flag).
        self.mouse = True
        #: The command that owns the current task panel, if any.
        self.sourceCmd = None
        #: Whether a task is running.
        self.isTaskOn = False
        #: Whether new geometry is construction geometry.
        self.isConstructionMode = False
        #: The autogroup (Draft Layer) name for new objects.
        self.autogroup = None
        #: Relative/global coordinate entry flags.
        self.globalMode = False
        self.relativeMode = True
        #: The workplane label and text-entry contents (UI placeholders).
        self.wplabel = None
        self.textValue = None
        #: Whether a closed wire becomes a face.
        self.makeFaceMode = True
        #: Whether the tool continues after a commit.
        self.continueCmd = False
        #: Upstream ``InitGui`` sets this once preferences are loaded.
        self.loadedPreferences = False
        #: Task-panel/Qt widgets (placeholders: no Qt in FerroCAD).
        self.tray = None
        self.taskUi = None
        self.panel = None
        self.panelName = None
        #: The last validated point and the constraint mask (upstream names).
        self.new_point = None
        self.last_point = None
        self.mask = None
        self.pointcallback = None

    def Activated(self):
        """Mark the task panel active and make it the Snapper's UI adapter."""
        self.isTaskOn = True
        Snapper.ui = self
        return None

    def Deactivated(self):
        """End the task, finishing a source command that owns the panel."""
        self.isTaskOn = False
        Snapper.clearPointConstraintProvider(self)
        command = self.sourceCmd
        self._finish_command(command)
        self.sourceCmd = None
        return None

    def offUi(self):
        """Release the panel's command and provider state."""
        Snapper.clearPointConstraintProvider(self)
        self.sourceCmd = None
        self.isTaskOn = False
        self.pointcallback = None
        self.textValue = None
        self.mask = None
        return None

    def _finish_command(self, command, cont=False):
        finish = getattr(command, "finish", None)
        if not callable(finish):
            return False
        try:
            finish(cont=cont)
        except TypeError:
            # A compatibility command may implement the older ``finish()``.
            finish()
        return True

    def validatePoint(self, point=None):
        """Apply the current constraint to ``point`` and return it.

        Without a point this is upstream's ``validatePoint()`` predicate and
        returns ``True``. With a point, the Snapper's active axis constraint is
        applied (when one is set) and then the registered point-constraint
        provider is consulted, exactly as the interactive panel does.
        """
        if point is None:
            return True
        world = _triple(point)
        if Snapper.basepoint is not None or Snapper.constraintAxis is not None:
            world = Snapper.constrain(world)
        provider = Snapper.pointConstraintProvider
        if provider is not None:
            has = getattr(provider, "has_point_constraints", None)
            if not callable(has) or has():
                constrained = provider.constrain_point(world, Snapper.basepoint)
                if constrained is not None:
                    world = _triple(constrained)
        self.new_point = world
        return world

    def escape(self):
        """Cancel the current task: end snapping and finish the source command."""
        Snapper.off()
        command = self.sourceCmd
        if command is not None:
            self._finish_command(command, cont=False)
        self.sourceCmd = None
        self.isTaskOn = False
        return None

    def setStyleButton(self, *_args, **_kwargs):
        """Raise: the Draft style button is a Qt widget FerroCAD does not build."""
        raise NotImplementedError(
            "draftToolBar.setStyleButton(): FerroCAD's headless task panel has "
            "no Qt style button; set the Draft style through FerroCAD.Grid/"
            "the style parameters instead"
        )

    def setAutoGroup(self, value=None):
        """Set (or clear) the autogroup name; returns the stored value."""
        self.autogroup = None if value is None else value
        return self.autogroup


#: The module-level singleton, exposed as ``FreeCADGui.draftToolBar``.
draftToolBar = _DraftToolBar()


# ---------------------------------------------------------------------------
# Hints and input values
# ---------------------------------------------------------------------------


class _HintManager:
    """The headless hint channel (``FreeCADGui.HintManager``).

    The shell reads ``FreeCADGui.draft.hint``; ``show`` stores the hints and
    mirrors their text there, ``hide`` clears both.
    """

    def __init__(self):
        self.hints = ()
        self.hint = ""

    def show(self, *hints):
        """Show ``hints`` and mirror their text into ``FreeCADGui.draft.hint``."""
        self.hints = tuple(hints)
        self.hint = "\n".join(str(hint) for hint in hints)
        FreeCADGui.draft.hint = self.hint
        return self.hint

    def hide(self):
        """Clear the last hint from the status channel."""
        self.hints = ()
        self.hint = ""
        FreeCADGui.draft.hint = ""
        return None

    @property
    def last(self):
        """The last hint passed to :meth:`show`, or ``None``."""
        return self.hints[-1] if self.hints else None

    #: Explicit alias for the most recent hint.
    @property
    def last_hint(self):
        return self.last


#: The module-level singleton, exposed as ``FreeCADGui.HintManager``.
HintManager = _HintManager()


class InputHint:
    """One status-bar hint: ``text`` plus any formatting ``args``."""

    __slots__ = ("text", "args")

    def __init__(self, text, *args):
        self.text = text
        self.args = tuple(args)

    def __eq__(self, other):
        if not isinstance(other, InputHint):
            return NotImplemented
        return (self.text, self.args) == (other.text, other.args)

    def __repr__(self):
        if not self.args:
            return f"InputHint({self.text!r})"
        arguments = ", ".join(repr(arg) for arg in self.args)
        return f"InputHint({self.text!r}, {arguments})"


class _UserInputSentinel:
    """A distinct, readable user-input event value."""

    __slots__ = ("name",)

    def __init__(self, name):
        self.name = name

    def __repr__(self):
        return f"UserInput.{self.name}"


class UserInput:
    """The small user-input vocabulary the task panels compare against."""

    MouseLeft = _UserInputSentinel("MouseLeft")
    MouseMiddle = _UserInputSentinel("MouseMiddle")
    MouseRight = _UserInputSentinel("MouseRight")
    MouseMove = _UserInputSentinel("MouseMove")
    KeyEscape = _UserInputSentinel("KeyEscape")
    KeyEnter = _UserInputSentinel("KeyEnter")
    KeyDelete = _UserInputSentinel("KeyDelete")


# ---------------------------------------------------------------------------
# The checkable snap-bar commands
# ---------------------------------------------------------------------------


class _SnapCommand:
    """One checkable ``Draft_Snap_*`` command."""

    #: The mode this command toggles (set on each concrete instance).
    mode = ""
    #: Human-facing action text.
    menu = ""
    #: Tooltip.
    tooltip = ""

    def __init__(self, mode, menu, tooltip):
        self.mode = mode
        self.menu = menu
        self.tooltip = tooltip

    def GetResources(self):
        return {
            "Pixmap": _COMMAND_IDS[self.mode],
            "MenuText": self.menu,
            # Under the header's `Snap` control the mode name alone is enough; the
            # full `Snap Midpoint` stays in the palette so it is unambiguous there.
            "FerroCAD_Short": self.mode,
            "ToolTip": self.tooltip,
            "CmdType": "NoTransaction",
            "Checkable": True,
            "FerroCAD_Surface": "snap",
            "FerroCAD_Parent": _SNAP_LOCK_ID,
        }

    def IsActive(self):
        """The bar is live whenever snapping is globally enabled."""
        return Snapper.isEnabled()

    def IsChecked(self):
        """Whether this mode is in the Snapper's active mask."""
        return Snapper.mode_enabled(self.mode)

    def Activated(self, status=None):
        """Toggle the mode; an explicit ``status`` sets it instead."""
        if status is None:
            return Snapper.toggle_snap(self.mode)
        return Snapper.toggle_snap(self.mode, bool(status))

    def __repr__(self):
        return f"<{_COMMAND_IDS[self.mode]} mode={self.mode!r}>"


class _SnapLockCommand:
    """``Draft_Snap_Lock``: the global snap switch (upstream's Snap Lock).

    The label of the header's ``Snap`` control runs this command, exactly as the
    ``Grid`` and ``Axes`` labels run theirs. The mode mask survives a lock, so
    unlocking restores the filters that were on before.
    """

    def GetResources(self):
        return {
            "Pixmap": _SNAP_LOCK_ID,
            "MenuText": "Snap Lock",
            "ToolTip": "Turn snapping on or off; the snap modes keep their own state",
            "CmdType": "NoTransaction",
            "Checkable": True,
            "FerroCAD_Surface": "snap",
        }

    def IsActive(self):
        return True

    def IsChecked(self):
        return Snapper.isEnabled()

    def Activated(self, status=None):
        if status is None:
            return Snapper.toggle_enabled()
        return Snapper.set_enabled(bool(status))

    def __repr__(self):
        return f"<{_SNAP_LOCK_ID} enabled={Snapper.isEnabled()}>"


def _command_specs():
    return (
        ("Grid", "Snap Grid", "Snaps to the intersections of grid lines"),
        ("Endpoint", "Snap Endpoint", "Snaps to the endpoints of edges"),
        ("Midpoint", "Snap Midpoint", "Snaps to the midpoint of edges"),
        ("Center", "Snap Center", "Snaps to the centre of a face or arc"),
        ("Origin", "Snap Origin", "Snaps to the workplane origin"),
        ("Vertex", "Snap Vertex", "Snaps to a topological vertex"),
    )


def _register_commands():
    """Register the snap lock and the six snap commands once (a reload must not raise)."""
    registered = set(FreeCADGui.listCommands())
    if _SNAP_LOCK_ID not in registered:
        FreeCADGui.addCommand(_SNAP_LOCK_ID, _SnapLockCommand())
    for mode, menu, tooltip in _command_specs():
        command_id = _COMMAND_IDS[mode]
        if command_id in registered:
            continue
        FreeCADGui.addCommand(command_id, _SnapCommand(mode, menu, tooltip))


_register_commands()


# ---------------------------------------------------------------------------
# Install the singletons on the facade
# ---------------------------------------------------------------------------


def _install():
    """Attach this module's public surface to ``FreeCADGui`` (idempotent)."""
    FreeCADGui.draft = sys.modules[__name__]
    Snapper.ui = draftToolBar
    draftToolBar.ui = draftToolBar
    FreeCADGui.Snapper = Snapper
    FreeCADGui.draftToolBar = draftToolBar
    FreeCADGui.HintManager = HintManager
    FreeCADGui.InputHint = InputHint
    FreeCADGui.UserInput = UserInput


_install()
