"""The native Draft tool layer: a ``PlaneGui`` and the Line/Wire commands.

This module is the FerroCAD half of Qt-reduction slice **Q4**. Upstream's Draft
workbench reaches its spatial state through ``WorkingPlane.PlaneGui``,
``FreeCADGui.draftToolBar`` and ``FreeCADGui.Snapper``; here that state is backed
by ``ferrocad_stage.Workplane`` and by the async point-input surface the shell
installs on ``FreeCADGui`` (``Snapper.getPoint`` + ``draft.pick``).

What this module provides
-------------------------

``PlaneGui``
    A native replacement for upstream's ``WorkingPlane.PlaneGui``. The frame is
    an immutable :class:`ferrocad.stage.Workplane`; ``get_placement`` turns it
    into a ``FreeCAD.Placement`` whose rotation columns are ``u``, ``v`` and
    ``n = u × v``, so a placement round-trips back into a workplane. The
    GUI-touching parts are gone: ``update_gui`` records a label for the status
    channel and never loads a Qt icon, and ``_update_grid`` returns the shell's
    ``{"view": {"grid_minor": …, "grid_major": …}}`` request instead of calling
    a view.

``Line`` / ``Wire``
    Small ``Creator``-shaped commands registered as ``Draft_Line`` and
    ``Draft_Wire``. They are driven by the Q4 async point session: ``Activated``
    opens a session, ``FreeCADGui.draft.pick(point)`` appends a node and shows
    an ``InputHint``, ``Enter``/``draft.finish()`` queues a ``doCommand`` commit
    through ``QtCore.QTimer.singleShot(0, …)``, and ``draft.escape()`` ends the
    session without touching the document.

Why the native tool rather than upstream's ``gui_lines.Line``
-------------------------------------------------------------

The upstream classes cannot run in this headless image, for four independent
reasons:

* ``Line.Activated`` writes ``obj.ViewObject.ShowInTree`` / ``Selectable`` and
  ``drawUpdate`` calls ``ViewObject.hide()``; ``ferrocad.ViewObject`` exposes
  none of them.
* It drives picking from ``view.addEventCallback("SoEvent", …)``, while Q4
  delivers points through ``Snapper.getPoint`` / ``draft.pick``.
* It imports ``Part`` and builds preview shapes during picking; the standalone
  ``ferrocad`` extension in this image does not link Part.
* Its ``finish`` queues ``Draft.make_wire``; ``python/Draft`` is the FerroCAD
  ``make_clone`` shim, which has no ``make_wire`` (and this module may not edit
  it).

``python/FerroCADGui/tools.py`` does not edit any of those files. It does
install, **only when the corresponding Q4 name is absent**, a minimal adapter
for ``draftToolBar``/``draft``/``HintManager``/``doCommand`` and adds
``make_line``/``make_wire`` to the ``Draft`` module at runtime, so the commands
are usable headlessly and the observable contract can be exercised. When the
shell's real Q4 surface is present, it is used unchanged.

Importing this module has these side effects:

* ``FreeCADGui.PlaneGui`` is set to :class:`PlaneGui`;
* ``Draft_Line`` and ``Draft_Wire`` are registered unless already present;
* ``Draft.make_line`` / ``Draft.make_wire`` are installed if the Draft module
  does not already provide them;
* missing Q4 adapter names are filled with the minimal fallbacks described above.
"""

from __future__ import annotations

import importlib
import importlib.util
import sys
from pathlib import Path

import FreeCAD
import FreeCADGui

__all__ = [
    "PlaneGui",
    "get_working_plane",
    "set_working_plane",
    "Creator",
    "Line",
    "Wire",
    "make_line",
    "make_wire",
    "register_commands",
    "get_command",
    "upstream_report",
]


# ---------------------------------------------------------------------------
# The stage extension (ferrocad.stage after install())
# ---------------------------------------------------------------------------


def _load_stage():
    """Return the ``ferrocad_stage`` module, installing the ``ferrocad.stage`` alias."""
    module = sys.modules.get("ferrocad_stage")
    if module is None:
        try:
            module = importlib.import_module("ferrocad_stage")
        except Exception:  # noqa: BLE001 - an optional build artefact
            module = None
    if module is not None:
        install = getattr(module, "install", None)
        if callable(install):
            try:
                install()
            except Exception:  # noqa: BLE001 - already installed is fine
                pass
    return module


_STAGE = _load_stage()


def _require_stage():
    if _STAGE is None:
        raise RuntimeError(
            "ferrocad.stage is unavailable: build and copy ferrocad_stage "
            "(see the module docstring)"
        )
    return _STAGE


def _is_workplane(value) -> bool:
    return _STAGE is not None and isinstance(value, _STAGE.Workplane)


# ---------------------------------------------------------------------------
# Vector helpers
# ---------------------------------------------------------------------------


def _vector(value) -> "FreeCAD.Vector":
    """Coerce a tuple/list/Vector to a ``FreeCAD.Vector``."""
    if isinstance(value, FreeCAD.Vector):
        return value
    return FreeCAD.Vector(*value)


def _triple(value):
    vector = _vector(value)
    return (vector.x, vector.y, vector.z)


def _normalized(value) -> "FreeCAD.Vector":
    vector = _vector(value)
    length = vector.Length
    if length <= 0.0:
        raise ValueError("a workplane normal must be non-zero")
    return FreeCAD.Vector(vector.x / length, vector.y / length, vector.z / length)


def _frame_from_normal(origin, normal):
    """Build a right-handed ``(u, v)`` pair for ``n = u × v``.

    The reference axis is ``+Z`` unless the normal is close to it, in which
    case ``+Y`` is used; ``u = reference × n`` then gives the canonical
    ``u = +X, v = +Y`` for the world normal ``+Z``.
    """
    n = _normalized(normal)
    reference = FreeCAD.Vector(0, 0, 1) if abs(n.z) < 0.9 else FreeCAD.Vector(0, 1, 0)
    u = reference.cross(n)
    length = u.Length
    if length <= 1e-12:
        reference = FreeCAD.Vector(1, 0, 0)
        u = reference.cross(n)
        length = u.Length
    u = FreeCAD.Vector(u.x / length, u.y / length, u.z / length)
    v = n.cross(u)
    return _triple(origin), _triple(u), _triple(v)


def _same(a, b, tolerance=1e-9) -> bool:
    return _vector(a).sub(_vector(b)).Length <= tolerance


# ---------------------------------------------------------------------------
# PlaneGui
# ---------------------------------------------------------------------------


class PlaneGui:
    """A native, upstream-shaped working plane over ``ferrocad.stage.Workplane``.

    Upstream's ``PlaneGui`` mixes the plane math with a history and a Qt icon
    button. This class keeps the members the Draft tools touch and drops the
    view coupling: there is no ``_view``, no ``QIcon`` and no direct shell call.
    """

    def __init__(
        self,
        u=None,
        v=None,
        w=None,
        pos=None,
        auto=True,
        label="World",
        icon=None,
        tip=None,
        workplane=None,
        grid_minor=1.0,
        grid_major=10.0,
    ):
        stage = _require_stage()

        if workplane is None and isinstance(u, PlaneGui):
            workplane = u.workplane
        elif workplane is None and _is_workplane(u):
            workplane = u

        if workplane is not None:
            if not _is_workplane(workplane):
                raise TypeError("plane must be a ferrocad.stage.Workplane")
            self._workplane = workplane
        else:
            origin = (0.0, 0.0, 0.0) if pos is None else _triple(pos)
            if u is None and v is None:
                normal = (0.0, 0.0, 1.0) if w is None else _triple(w)
                origin, u_triple, v_triple = _frame_from_normal(origin, normal)
            elif u is None:
                n = _normalized(w if w is not None else (0.0, 0.0, 1.0))
                v_triple = _triple(v)
                u_vec = _vector(v_triple).cross(n)
                u_triple = _triple(_normalized(u_vec))
                v_triple = _triple(_normalized(v_triple))
            elif v is None:
                n = _normalized(w if w is not None else (0.0, 0.0, 1.0))
                u_triple = _triple(_normalized(u))
                v_triple = _triple(n.cross(_vector(u_triple)))
            else:
                u_triple, v_triple = _triple(u), _triple(v)
            self._workplane = stage.Workplane(origin, u_triple, v_triple)

        self.auto = bool(auto)
        self.label = label
        #: No Qt icon is ever resolved; ``update_gui`` records the label only.
        self.icon = None
        self.icon_path = None
        self.tip = tip
        self.grid_minor = float(grid_minor)
        self.grid_major = float(grid_major)
        self.status_label = None
        self.last_label = None
        self.last_grid_request = None
        self._stored = None

    # -- the frame ---------------------------------------------------------

    @property
    def workplane(self):
        return self._workplane

    def get_working_plane(self):
        """The current :class:`ferrocad.stage.Workplane`."""
        return self._workplane

    def set_working_plane(self, workplane):
        """Replace the frame with ``workplane`` (a Workplane or a PlaneGui)."""
        if isinstance(workplane, PlaneGui):
            workplane = workplane.workplane
        if not _is_workplane(workplane):
            raise TypeError("plane must be a ferrocad.stage.Workplane")
        self._workplane = workplane
        self._update_grid()
        self.update_gui()
        return self

    def get_placement(self):
        """A ``FreeCAD.Placement`` whose ``+Z`` is the workplane normal.

        The rotation's first two columns are ``u`` and ``v`` and its third is
        ``n = u × v``; ``Placement.Rotation.multVec`` therefore recovers the
        same axes and a ``Workplane`` built from them is identical.
        """
        wp = self._workplane
        ux, uy, uz = wp.u
        vx, vy, vz = wp.v
        nx, ny, nz = wp.n
        matrix = FreeCAD.Matrix()
        matrix.A11, matrix.A21, matrix.A31 = ux, uy, uz
        matrix.A12, matrix.A22, matrix.A32 = vx, vy, vz
        matrix.A13, matrix.A23, matrix.A33 = nx, ny, nz
        return FreeCAD.Placement(
            FreeCAD.Vector(*wp.origin), FreeCAD.Rotation(matrix)
        )

    def getRotation(self):
        """The placement's rotation (upstream's ``PlaneBase.getRotation``)."""
        return self.get_placement().Rotation

    # -- alignment ---------------------------------------------------------

    def align_to_face(self, origin, normal=None, offset=0):
        """Align to a planar face, or directly to ``(origin, normal)``.

        ``origin`` may be a face (with ``CenterOfMass`` and ``normalAt``) when
        ``normal`` is omitted, matching upstream's one-argument form.
        """
        stage = _require_stage()
        if normal is None:
            face = origin
            origin = getattr(face, "CenterOfMass", None)
            if origin is None:
                origin = getattr(face, "centerOfMass", None)
            if origin is None:
                raise TypeError("align_to_face needs an origin and a normal")
            normal_at = getattr(face, "normalAt", None)
            if not callable(normal_at):
                raise TypeError("the face has no normalAt(); pass a normal")
            normal = normal_at(0, 0)

        n = _normalized(normal)
        base = _vector(origin).add(n.scale(offset)) if offset else _vector(origin)
        o, u, v = _frame_from_normal(base, n)
        self._workplane = stage.Workplane(o, u, v)
        self.auto = False
        self._update_grid()
        self.update_gui()
        return self

    def align_to_view(self, direction):
        """Make ``direction`` the normal of a frame centred on the origin."""
        stage = _require_stage()
        o, u, v = _frame_from_normal((0.0, 0.0, 0.0), direction)
        self._workplane = stage.Workplane(o, u, v)
        self.auto = False
        self._update_grid()
        self.update_gui()
        return self

    def align_to_point_and_axis(self, point, axis, offset=0, upvec=None):
        """Align to a point and an axis (the upstream-facing form)."""
        if upvec is None:
            return self.align_to_face(point, axis, offset=offset)
        stage = _require_stage()
        n = _normalized(axis)
        u = _vector(upvec)
        u = u.sub(n.scale(u.dot(n)))
        if u.Length <= 1e-12:
            return self.align_to_face(point, n, offset=offset)
        u = _normalized(u)
        v = n.cross(u)
        base = _vector(point).add(n.scale(offset)) if offset else _vector(point)
        self._workplane = stage.Workplane(_triple(base), _triple(u), _triple(v))
        self.auto = False
        self._update_grid()
        self.update_gui()
        return self

    def reset(self):
        """Return to the world ``XY`` plane."""
        stage = _require_stage()
        self._workplane = stage.Workplane.world()
        self.auto = True
        self._update_grid()
        self.update_gui()
        return self

    # -- shell requests ----------------------------------------------------

    def _update_grid(self):
        """Return the shell's GridSpec-equivalent ``{"view": …}`` request.

        The request is returned, never applied: ``PlaneGui`` does not call the
        active view or the shell.
        """
        request = {
            "view": {
                "grid_minor": float(self.grid_minor),
                "grid_major": float(self.grid_major),
            }
        }
        self.last_grid_request = request
        return request

    def update_gui(self):
        """Record the label for the status channel; never touch a Qt icon."""
        self.status_label = self.label
        self.last_label = self.label
        return None

    def set_to_view(self, offset=0, center_on_view=False):
        """Ask the shell to derive the workplane from the current view."""
        return {"view": {"workplane": "view"}}

    # -- upstream-facing compatibility ------------------------------------

    def _save(self):
        self._stored = self._workplane

    def _restore(self):
        if self._stored is not None:
            self._workplane = self._stored
            self._stored = None
            self._update_grid()
            self.update_gui()

    def _update_all(self, _hist_add=True):
        self._update_grid()
        self.update_gui()

    def auto_align(self):
        return None

    def set_to_default(self):
        return self.reset()

    def copy(self):
        clone = PlaneGui(workplane=self._workplane)
        clone.label = self.label
        clone.tip = self.tip
        clone.auto = self.auto
        clone.grid_minor = self.grid_minor
        clone.grid_major = self.grid_major
        return clone

    def is_global(self):
        wp = self._workplane
        return _same(wp.origin, (0, 0, 0)) and _same(wp.n, (0, 0, 1))

    @property
    def u(self):
        return FreeCAD.Vector(*self._workplane.u)

    @property
    def v(self):
        return FreeCAD.Vector(*self._workplane.v)

    @property
    def axis(self):
        return FreeCAD.Vector(*self._workplane.n)

    @property
    def position(self):
        return FreeCAD.Vector(*self._workplane.origin)

    def get_parameters(self):
        return {
            "u": self.u,
            "v": self.v,
            "axis": self.axis,
            "position": self.position,
            "auto": self.auto,
        }

    def set_parameters(self, parameters):
        stage = _require_stage()
        self._workplane = stage.Workplane(
            _triple(parameters["position"]),
            _triple(parameters["u"]),
            _triple(parameters["v"]),
        )
        self.auto = bool(parameters.get("auto", self.auto))
        return self


#: The process-wide working plane, mirroring upstream's module-level cache.
_ACTIVE_PLANE = None


def get_working_plane(update=True):
    """Return the current :class:`PlaneGui` (creating the world plane once)."""
    global _ACTIVE_PLANE
    if _ACTIVE_PLANE is None:
        _ACTIVE_PLANE = PlaneGui()
    return _ACTIVE_PLANE


def set_working_plane(workplane):
    """Set the process-wide frame and return its :class:`PlaneGui`."""
    global _ACTIVE_PLANE
    if workplane is None:
        _require_stage()
        workplane = _STAGE.Workplane.world()
    if _ACTIVE_PLANE is None:
        _ACTIVE_PLANE = PlaneGui(workplane=workplane)
    else:
        _ACTIVE_PLANE.set_working_plane(workplane)
    return _ACTIVE_PLANE


def reroute_upstream_working_plane():
    """Point upstream's ``WorkingPlane.PlaneGui`` at this class, if importable.

    Returns ``True`` when the upstream module was found. The conformance
    harness can call this after it adds ``src/Mod/Draft`` to ``sys.path``; the
    normal application path does not import upstream ``WorkingPlane``.
    """
    try:
        module = importlib.import_module("WorkingPlane")
    except Exception:  # noqa: BLE001 - the checkout is optional
        return False
    module.PlaneGui = PlaneGui
    module.get_working_plane = get_working_plane
    return True


# ---------------------------------------------------------------------------
# Native Draft creation primitives (also installed on the Draft module)
# ---------------------------------------------------------------------------


def _active_document():
    document = FreeCAD.ActiveDocument
    if document is None:
        raise RuntimeError(
            "no active document: open or create one before creating a Draft object"
        )
    return document


def make_wire(points, closed=False, placement=None, face=None, support=None, bs2wire=False):
    """Create one ``Part::Feature`` polyline from ``points``.

    This is the native stand-in for ``Draft.make_wire``. It creates exactly one
    document object, so a cancelled session leaves the document unchanged.
    ``face``/``support`` are accepted for signature compatibility.
    """
    document = _active_document()
    try:
        import Part
    except Exception as exc:  # noqa: BLE001 - Part is optional in a bare image
        raise RuntimeError(
            "Draft wire creation needs the Part module (%s)" % (exc,)
        ) from exc

    coordinates = [_vector(point) for point in points]
    if closed and coordinates and not _same(coordinates[0], coordinates[-1]):
        coordinates.append(coordinates[0])
    if placement is not None:
        inverse = placement.inverse()
        coordinates = [inverse.multVec(point) for point in coordinates]

    shape = Part.makePolygon(coordinates)
    name = "Line" if len(coordinates) == 2 and not closed else "Wire"
    obj = document.addObject("Part::Feature", name)
    obj.Shape = shape
    if placement is not None:
        obj.Placement = placement
    document.recompute()
    return obj


def make_line(first_param, last_param=None):
    """Create one line from two points (or from a point-bearing shape)."""
    if last_param is not None:
        first, last = first_param, last_param
    elif hasattr(first_param, "StartPoint") and hasattr(first_param, "EndPoint"):
        first, last = first_param.StartPoint, first_param.EndPoint
    elif hasattr(first_param, "Vertexes") and first_param.Vertexes:
        first = first_param.Vertexes[0].Point
        last = first_param.Vertexes[-1].Point
    elif isinstance(first_param, (list, tuple)) and len(first_param) == 2:
        first, last = first_param
    else:
        raise TypeError("make_line needs two points or a two-vertex shape")
    return make_wire([_vector(first), _vector(last)], closed=False)


def _install_draft_creation_functions():
    """Add ``make_line``/``make_wire`` to ``Draft`` when it does not have them."""
    module = sys.modules.get("Draft")
    if module is None:
        try:
            module = importlib.import_module("Draft")
        except Exception:  # noqa: BLE001 - no Draft shim is a valid headless tree
            return False
    if not callable(getattr(module, "make_wire", None)):
        setattr(module, "make_wire", make_wire)
    if not callable(getattr(module, "make_line", None)):
        setattr(module, "make_line", make_line)
    return True


# ---------------------------------------------------------------------------
# Minimal Q4 adapter fallbacks (installed only when the shell has not supplied)
# ---------------------------------------------------------------------------


class _UserInput:
    MouseLeft = "MouseLeft"
    MouseRight = "MouseRight"
    KeyEnter = "KeyEnter"
    KeyEscape = "KeyEscape"
    KeyShift = "KeyShift"
    KeyControl = "KeyControl"
    KeyAlt = "KeyAlt"


class _InputHint:
    """A tiny ``FreeCADGui.InputHint`` stand-in with the same payload shape."""

    def __init__(self, text, *args):
        self.text = text
        self.args = args

    def __repr__(self):
        return "InputHint(%r%s)" % (
            self.text,
            "".join(", %r" % (arg,) for arg in self.args),
        )


class _HintManager:
    """Records hints so a headless session can inspect the last one."""

    shown = []
    last = None

    @classmethod
    def show(cls, *hints):
        cls.shown.append(hints)
        cls.last = hints
        return None

    @classmethod
    def hide(cls):
        cls.last = None
        return None


class _ToolBar:
    """The fields the native tools touch, with no widgets behind them."""

    def __init__(self):
        self.mouse = False
        self.sourceCmd = None
        self.isTaskOn = False
        self.autogroup = False
        self.wplabel = None
        self.textValue = ""
        self.makeFaceMode = False
        self.continueCmd = False
        self.continueMode = False
        self.loadedPreferences = False
        self.tray = None
        self.taskUi = None

    def Activated(self):
        self.isTaskOn = True

    def Deactivated(self):
        self.isTaskOn = False

    def offUi(self):
        self.isTaskOn = False

    def validatePoint(self, *args, **kwargs):
        return True

    def escape(self):
        return None

    def setStyleButton(self, *args, **kwargs):
        return None

    def setAutoGroup(self, *args, **kwargs):
        return None

    def lineUi(self, *args, **kwargs):
        self.isTaskOn = True
        return self

    def wireUi(self, *args, **kwargs):
        self.isTaskOn = True
        return self

    def redraw(self):
        return None

    def setNextFocus(self):
        return None

    def isConstructionMode(self):
        return False


class _DraftSession:
    """The async point session used when ``FreeCADGui.draft`` is absent.

    It mirrors the Q4 contract: ``getPoint`` stores a one-shot callback,
    ``pick`` delivers a point to it, and ``finish``/``escape`` operate on the
    active tool.
    """

    def __init__(self):
        self._callback = None

    def getPoint(self, callback, *args):
        self._callback = callback
        return None

    def pick(self, point):
        callback = self._callback
        self._callback = None
        if callback is not None:
            return callback(point)
        return None

    def finish(self):
        tool = _ACTIVE_TOOL
        if tool is not None:
            return tool.finish()
        return None

    def escape(self):
        self._callback = None
        tool = _ACTIVE_TOOL
        if tool is not None:
            return tool.escape()
        return None


def _fallback_do_command(text):
    """Execute ``text`` with the names an upstream ``doCommand`` expects."""
    namespace = {
        "__name__": "__ferrocad_draft_command__",
        "FreeCAD": FreeCAD,
        "App": FreeCAD,
    }
    draft = sys.modules.get("Draft")
    if draft is not None:
        namespace["Draft"] = draft
    exec(text, namespace)  # noqa: S102 - the upstream command protocol
    return None


def _fallback_add_module(name):
    module = importlib.import_module(name)
    setattr(FreeCADGui, name, module)
    return module


def _install_fallbacks():
    if not hasattr(FreeCADGui, "UserInput"):
        FreeCADGui.UserInput = _UserInput
    if not hasattr(FreeCADGui, "InputHint"):
        FreeCADGui.InputHint = _InputHint
    if not hasattr(FreeCADGui, "HintManager"):
        FreeCADGui.HintManager = _HintManager
    if not hasattr(FreeCADGui, "draftToolBar"):
        FreeCADGui.draftToolBar = _ToolBar()
    if not hasattr(FreeCADGui, "draft"):
        FreeCADGui.draft = _DraftSession()
    if not hasattr(FreeCADGui, "doCommand"):
        FreeCADGui.doCommand = _fallback_do_command
    if not hasattr(FreeCADGui, "addModule"):
        FreeCADGui.addModule = _fallback_add_module


# ---------------------------------------------------------------------------
# The native Creator-shaped commands
# ---------------------------------------------------------------------------


_ACTIVE_TOOL = None


def _active_tool():
    return _ACTIVE_TOOL


class Creator:
    """A point-session creation tool in the shape upstream's ``Creator`` uses.

    Subclasses set ``mode`` to ``"line"`` or ``"wire"``. A session is active
    between :meth:`Activated` and :meth:`finish`/:meth:`escape`; points arrive
    through the Q4 async callback installed by ``Snapper.getPoint``.
    """

    mode = "line"
    feature_name = "Line"

    def __init__(self, mode=None, feature_name=None):
        if mode is not None:
            self.mode = mode
        if feature_name is not None:
            self.feature_name = feature_name
        self.node = []
        self.point = None
        self.doc = None
        self.ui = None
        self.wp = None
        self.is_active = False
        self.hints = []
        self.last_hint = None
        self.events = []
        self.commit_text = None
        self._closed = True
        self._input_owner = None

    # -- command protocol --------------------------------------------------

    def IsActive(self):
        return FreeCAD.ActiveDocument is not None

    def GetResources(self):
        return {
            "Pixmap": "Draft_" + self.feature_name,
            "MenuText": self.feature_name,
            "ToolTip": "Creates a Draft " + self.feature_name.lower(),
        }

    def Activated(self, name=None, icon=None, task_title=None):
        global _ACTIVE_TOOL

        previous = getattr(FreeCAD, "activeDraftCommand", None)
        if previous is not None and previous is not self:
            try:
                previous.finish()
            except Exception:  # noqa: BLE001 - a stale tool must not block this one
                pass

        FreeCAD.activeDraftCommand = self
        _ACTIVE_TOOL = self

        self.doc = FreeCAD.ActiveDocument
        self.node = []
        self.point = None
        self.hints = []
        self.last_hint = None
        self.events = ["activate"]
        self.commit_text = None
        self._closed = False
        self.is_active = True

        self.wp = get_working_plane()
        self.ui = getattr(FreeCADGui, "draftToolBar", None)
        if self.ui is not None:
            self.ui.sourceCmd = self
            try:
                self.ui.mouse = True
            except Exception:  # noqa: BLE001 - optional native field
                pass

        self._register_point_input()
        self._show_hint(self._next_hint())
        return None

    def finish(self, cont=False, closed=False):
        """Commit the collected points through ``doCommand`` and end the session."""
        global _ACTIVE_TOOL
        if self._closed:
            return None
        self._closed = True
        self.is_active = False
        self.events.append("finish")
        self._clear_ui()
        FreeCAD.activeDraftCommand = None
        if _ACTIVE_TOOL is self:
            _ACTIVE_TOOL = None
        self._snapper_off()
        self._hide_hints()

        text = None
        if len(self.node) >= 2 and self.doc is not None:
            text = self._command_text(closed)
            self.commit_text = text
            self._schedule_commit(text)
        self.node = []
        self.point = None
        if cont:
            # Continuation re-opens the session without a second commit.
            self.Activated()
        return text

    def escape(self):
        """End the session without creating anything."""
        global _ACTIVE_TOOL
        if self._closed:
            return None
        self._closed = True
        self.is_active = False
        self.events.append("escape")
        self.node = []
        self.point = None
        self._clear_ui()
        FreeCAD.activeDraftCommand = None
        if _ACTIVE_TOOL is self:
            _ACTIVE_TOOL = None
        self._snapper_off()
        self._hide_hints()
        return None

    # -- point input -------------------------------------------------------

    def pick(self, point):
        """Append one picked point (the public half of the point callback)."""
        return self._on_pick(point)

    def _on_pick(self, point, *args):
        if self._closed:
            return None
        if point is None:
            return self.escape()
        candidate = _vector(point)
        if self.node and _same(self.node[-1], candidate):
            self._show_hint("Point identical to previous point")
            self._register_point_input()
            return None
        self.node.append(candidate)
        self.point = candidate
        self.events.append("pick")
        self._show_hint(self._next_hint())
        self._register_point_input()
        return candidate

    def _register_point_input(self):
        callback = self._on_pick
        candidates = [
            getattr(FreeCADGui, "Snapper", None),
            getattr(FreeCADGui, "draft", None),
        ]
        for owner in candidates:
            get_point = getattr(owner, "getPoint", None)
            if not callable(get_point):
                continue
            try:
                # The session hooks are how Escape/Enter reach this tool; an
                # adapter without them still delivers points.
                get_point(callback, on_cancel=self.escape, on_finish=self.finish)
            except TypeError:
                try:
                    get_point(callback)
                except Exception:  # noqa: BLE001 - try the next input owner
                    continue
            except Exception:  # noqa: BLE001 - try the next input owner
                continue
            self._input_owner = owner
            return owner
        return None

    # -- hints -------------------------------------------------------------

    def _next_hint(self):
        if not self.node:
            return "Pick first point"
        if self.mode == "wire":
            if len(self.node) == 1:
                return "Pick next point"
            return "Pick next point, Enter to finish"
        if len(self.node) == 1:
            return "Pick second point"
        return "Enter to finish"

    def _show_hint(self, text):
        self.hints.append(text)
        self.last_hint = text
        hint_factory = getattr(FreeCADGui, "InputHint", None)
        user_input = getattr(FreeCADGui, "UserInput", None)
        mouse = getattr(user_input, "MouseLeft", None)
        hint = hint_factory(text, mouse) if callable(hint_factory) else text
        manager = getattr(FreeCADGui, "HintManager", None)
        show = getattr(manager, "show", None)
        if callable(show):
            try:
                show(hint)
            except Exception:  # noqa: BLE001 - the status channel is optional
                pass
        return hint

    def _hide_hints(self):
        manager = getattr(FreeCADGui, "HintManager", None)
        hide = getattr(manager, "hide", None)
        if callable(hide):
            try:
                hide()
            except Exception:  # noqa: BLE001 - the status channel is optional
                pass

    # -- finish helpers ----------------------------------------------------

    def _clear_ui(self):
        if self.ui is not None:
            self.ui.sourceCmd = None
            try:
                self.ui.offUi()
            except Exception:  # noqa: BLE001 - optional adapter method
                pass

    def _snapper_off(self):
        """End the Snapper session (its pending request is this tool's own)."""
        snapper = getattr(FreeCADGui, "Snapper", None)
        off = getattr(snapper, "off", None)
        if callable(off):
            try:
                off()
            except Exception:  # noqa: BLE001 - optional adapter method
                pass

    def _schedule_commit(self, text):
        self.events.append("queue")

        def run():
            self.events.append("commit")
            do_command = getattr(FreeCADGui, "doCommand", None)
            if callable(do_command):
                return do_command(text)
            return _fallback_do_command(text)

        try:
            from PySide.QtCore import QTimer
        except Exception:  # noqa: BLE001 - no Qt stub means commit immediately
            return run()
        QTimer.singleShot(0, run)
        return None

    def _command_text(self, closed):
        points = ", ".join(
            "FreeCAD.Vector(%r, %r, %r)" % (p.x, p.y, p.z) for p in self.node
        )
        if self.mode == "wire":
            return (
                "import FreeCAD, Draft; "
                "Draft.make_wire([%s], closed=%r, face=False, "
                "placement=None, support=None)" % (points, bool(closed))
            )
        first, last = self.node[0], self.node[-1]
        return (
            "import FreeCAD, Draft; "
            "Draft.make_line(FreeCAD.Vector(%r, %r, %r), "
            "FreeCAD.Vector(%r, %r, %r))"
            % (first.x, first.y, first.z, last.x, last.y, last.z)
        )


class Line(Creator):
    """``Draft_Line``: collect points and commit a two-point line on Enter."""

    mode = "line"
    feature_name = "Line"

    def __init__(self):
        super().__init__(mode="line", feature_name="Line")


class Wire(Creator):
    """``Draft_Wire``: collect points and commit a polyline on Enter."""

    mode = "wire"
    feature_name = "Wire"

    def __init__(self):
        super().__init__(mode="wire", feature_name="Wire")


# ---------------------------------------------------------------------------
# Registration
# ---------------------------------------------------------------------------


_COMMAND_INSTANCES = {}


def register_commands(replace=False):
    """Register ``Draft_Line`` and ``Draft_Wire``; return the instances by id.

    With ``replace=False`` (the import-time default) an existing registration —
    for example upstream ``gui_lines``' own — is left untouched. Tests and a
    workbench that wants the native tools can pass ``replace=True``.
    """
    existing = set(FreeCADGui.listCommands())
    registered = {}
    for name, factory in (("Draft_Line", Line), ("Draft_Wire", Wire)):
        if name in existing and not replace:
            registered[name] = _COMMAND_INSTANCES.get(name)
            continue
        if name in existing:
            FreeCADGui.removeCommand(name)
        command = factory()
        FreeCADGui.addCommand(name, command)
        _COMMAND_INSTANCES[name] = command
        registered[name] = command
    return registered


def get_command(name):
    """The command instance this module registered under ``name`` (or None)."""
    return _COMMAND_INSTANCES.get(name)


# ---------------------------------------------------------------------------
# Diagnostics
# ---------------------------------------------------------------------------


def upstream_report(root=None):
    """Report whether upstream's Draft checkout is present and what blocks it.

    This is intentionally a *report*, not an import: importing
    ``gui_lines`` registers ``Draft_Line``/``Draft_Wire`` and needs the Q4
    surface. The blockers are the reasons this module ships a native tool.
    """
    if root is None:
        root = Path(__file__).resolve().parents[2].parent / "freecad-upstream"
    else:
        root = Path(root)
    lines = root / "src" / "Mod" / "Draft" / "draftguitools" / "gui_lines.py"
    present = lines.is_file()

    try:
        part_available = importlib.util.find_spec("Part") is not None
    except Exception:  # noqa: BLE001 - a broken Part spec still means unavailable
        part_available = False

    blockers = [
        "upstream Line.Activated writes ViewObject.ShowInTree/Selectable and "
        "drawUpdate calls ViewObject.hide(); ferrocad.ViewObject has none",
        "upstream picking is SoEvent-driven; Q4 delivers points through "
        "Snapper.getPoint/draft.pick",
        "upstream finish queues Draft.make_wire; python/Draft is the make_clone "
        "shim and this module may not edit it",
    ]
    if not part_available:
        blockers.append("Part is not importable in this image")

    return {
        "checkout": str(root),
        "present": present,
        "part": part_available,
        "blockers": blockers,
        "path": "native",
    }


# ---------------------------------------------------------------------------
# Import-time installation
# ---------------------------------------------------------------------------

FreeCADGui.PlaneGui = PlaneGui
_install_fallbacks()
_install_draft_creation_functions()
register_commands()
# If the upstream checkout is already on ``sys.path`` (the conformance harness),
# point its ``WorkingPlane`` module at the native class too.
reroute_upstream_working_plane()
