"""The semantic gizmo API, exposed as ``FerroCAD.Gizmo``.

This module is private: the public form is ``FerroCAD.Gizmo`` (the singleton the
package binds lazily), not ``import FerroCAD._gizmo``. The settings live in the
native gizmo provider's parameter group,
``FreeCAD.ParamGet("User parameter:BaseApp/Preferences/Mods/gizmo/ferrocad")``,
under one typed key per :data:`GIZMO_SETTINGS_SCHEMA` entry, so the attribute
API and ``Settings/Gizmo`` are two views of the same store. The facade keeps no
state of its own.

The gizmo capability is new, so there is no legacy key to migrate: the store is
the provider group alone, and :meth:`_GizmoSettings.reset` clears only that
group.

``apply()`` builds the ``{"view": …}`` request a command returns. The shell owns
the viewport, so a property write changes the *desired* policy, and applying it
(through a command) is what repaints. The request also names the backend the
shell should ask to draw the gizmo: the active ``"gizmo"`` provider when a
registry is available, else this module's native ``"ferrocad"``. Headless
(``FreeCAD.GuiUp == 0``) every setter still validates and stores, and
``apply()`` returns the request without raising.
"""

from __future__ import annotations

import math

import FreeCAD

#: The provider feature and backend whose parameter group is the gizmo store.
FEATURE = "gizmo"
BACKEND = "ferrocad"

#: The parameter group the gizmo values live in (the native provider's group).
PREFERENCES = "User parameter:BaseApp/Preferences/Mods/%s/%s" % (FEATURE, BACKEND)

DEFAULT_ENABLED = True
DEFAULT_ACTIVE_COLOR = "#38bdf8"
DEFAULT_INACTIVE_COLOR = "#94a3b8"
#: The halo behind the handles. It is the WCAG 1.4.11 non-text-contrast aid: the
#: accent and the slate tokens are close in luminance to a mid-blue model, so a
#: dark outline is what keeps a handle readable over it.
DEFAULT_OUTLINE_COLOR = "#0f172a"
DEFAULT_HALO_WIDTH = 1.5
DEFAULT_SIZE = 80.0
DEFAULT_WIDTH = 1.5
#: The hover cue is a wider stroke as well as the accent colour (WCAG 1.4.1: use of
#: colour alone is not enough).
DEFAULT_HOVER_SCALE = 1.5
#: 12 px radius = a 24 × 24 px target (WCAG 2.5.8).
DEFAULT_HIT_RADIUS_PX = 12.0
MIN_HIT_RADIUS_PX = 12.0
DEFAULT_OPACITY = 1.0
#: Above half, an occluded handle reads as a front handle; the setting is clamped.
DEFAULT_GHOST_ALPHA = 0.20
MAX_GHOST_ALPHA = 0.5
DEFAULT_RING_SNAP_DEG = 15.0
#: The detent is stepped in 5° increments; `0` means continuous rotation.
RING_SNAP_STEP_DEG = 5.0
DEFAULT_SHOW_READOUT = True

#: The canonical gizmo schema. It is the one description of the manipulator
#: gizmo's knobs: ``FerroCADGui.registerProvider(..., settings=SETTINGS_SCHEMA)``
#: advertises these rows, and :class:`_GizmoSettings` stores each attribute under
#: the entry's ``name`` in :data:`PREFERENCES`. The kinds are the ones the
#: property editor understands.
SETTINGS_SCHEMA = (
    {
        "name": "enabled",
        "label": "Show gizmos",
        "kind": "bool",
        "default": DEFAULT_ENABLED,
    },
    {
        "name": "active_color",
        "label": "Active handle",
        "kind": "color",
        "default": DEFAULT_ACTIVE_COLOR,
    },
    {
        "name": "inactive_color",
        "label": "Inactive handle",
        "kind": "color",
        "default": DEFAULT_INACTIVE_COLOR,
    },
    {
        "name": "outline_color",
        "label": "Handle outline",
        "kind": "color",
        "default": DEFAULT_OUTLINE_COLOR,
    },
    {
        "name": "size",
        "label": "Handle size (px)",
        "kind": "float",
        "default": DEFAULT_SIZE,
        "min": 8.0,
        "max": 400.0,
    },
    {
        "name": "width",
        "label": "Stroke width (px)",
        "kind": "float",
        "default": DEFAULT_WIDTH,
        "min": 0.5,
        "max": 6.0,
    },
    {
        "name": "halo_width",
        "label": "Outline width (px)",
        "kind": "float",
        "default": DEFAULT_HALO_WIDTH,
        "min": 0.0,
        "max": 8.0,
    },
    {
        "name": "hover_scale",
        "label": "Hover stroke scale",
        "kind": "float",
        "default": DEFAULT_HOVER_SCALE,
        "min": 1.0,
        "max": 4.0,
    },
    {
        "name": "hit_radius_px",
        "label": "Grab radius (px)",
        "kind": "float",
        "default": DEFAULT_HIT_RADIUS_PX,
        "min": MIN_HIT_RADIUS_PX,
        "max": 48.0,
    },
    {
        "name": "opacity",
        "label": "Opacity",
        "kind": "float",
        "default": DEFAULT_OPACITY,
        "min": 0.0,
        "max": 1.0,
    },
    {
        "name": "ghost_alpha",
        "label": "Occluded alpha",
        "kind": "float",
        "default": DEFAULT_GHOST_ALPHA,
        "min": 0.0,
        "max": MAX_GHOST_ALPHA,
    },
    {
        "name": "ring_snap_deg",
        "label": "Rotation snap (deg)",
        "kind": "float",
        "default": DEFAULT_RING_SNAP_DEG,
        "min": 0.0,
        "max": 180.0,
        "step": RING_SNAP_STEP_DEG,
    },
    {
        "name": "show_readout",
        "label": "Show readout",
        "kind": "bool",
        "default": DEFAULT_SHOW_READOUT,
    },
)

#: The same schema under the explicit name the gizmo provider advertises.
GIZMO_SETTINGS_SCHEMA = SETTINGS_SCHEMA

_SPECS = {entry["name"]: entry for entry in SETTINGS_SCHEMA}

_HEX_DIGITS = "0123456789abcdefABCDEF"


def _prefs():
    """A fresh handle on the gizmo provider's parameter group."""
    return FreeCAD.ParamGet(PREFERENCES)


def _store(group, spec: dict, value) -> None:
    """Write one value under the schema entry's name and kind."""
    name = spec["name"]
    kind = spec["kind"]
    if kind == "bool":
        group.SetBool(name, bool(value))
    elif kind in ("float", "quantity"):
        group.SetFloat(name, float(value))
    else:
        group.SetString(name, str(value))


def _read(spec: dict):
    """One schema value from the provider group."""
    group = _prefs()
    name = spec["name"]
    kind = spec["kind"]
    default = spec["default"]
    if kind == "bool":
        return group.GetBool(name, bool(default))
    if kind in ("float", "quantity"):
        return group.GetFloat(name, float(default))
    return group.GetString(name, str(default))


def _write(spec: dict, value) -> None:
    """Store one schema value in the provider group."""
    _store(_prefs(), spec, value)


def _colour(value) -> str:
    """Validate a colour and return its canonical lowercase ``#rrggbb``."""
    if not isinstance(value, str):
        raise ValueError("a gizmo colour must be a '#rrggbb' string, got %r" % (value,))
    text = value.strip()
    if not text.startswith("#"):
        raise ValueError("a gizmo colour must start with '#', got %r" % (text,))
    digits = text[1:]
    if len(digits) == 3:
        digits = "".join(digit * 2 for digit in digits)
    elif len(digits) != 6:
        raise ValueError("a gizmo colour must be #rgb or #rrggbb, got %r" % (text,))
    if any(digit not in _HEX_DIGITS for digit in digits):
        raise ValueError("a gizmo colour has a non-hex digit, got %r" % (text,))
    return "#" + digits.lower()


def _number(value, name: str, *, minimum=0.0, maximum=None) -> float:
    """Validate a finite number in ``minimum..maximum``; never clamp it."""
    try:
        number = float(value)
    except (TypeError, ValueError):
        raise ValueError("%s must be a number, got %r" % (name, value)) from None
    if not math.isfinite(number):
        raise ValueError("%s must be finite, got %r" % (name, value))
    if number < minimum:
        raise ValueError("%s must be at least %s, got %r" % (name, minimum, value))
    if maximum is not None and number > maximum:
        raise ValueError("%s must be at most %s, got %r" % (name, maximum, value))
    return number


def _measured(spec: dict, value, name: str) -> float:
    """A schema float, validated against the entry's declared bounds."""
    return _number(
        value,
        name,
        minimum=float(spec.get("min", 0.0)),
        maximum=None if "max" not in spec else float(spec["max"]),
    )


def _ring_snap(value) -> float:
    """Validate the rotation snap and step it to the nearest 5° detent.

    ``0`` means continuous rotation. The rounding is half-up (``floor(x + 0.5)``)
    rather than Python's banker's ``round`` so it matches the Rust view's
    ``f64::round`` exactly; both operands are non-negative here.
    """
    number = _number(value, "the rotation snap", minimum=0.0, maximum=180.0)
    steps = math.floor(number / RING_SNAP_STEP_DEG + 0.5)
    return steps * RING_SNAP_STEP_DEG


def _backend() -> str:
    """The backend an ``apply()`` request should name.

    The provider registry is the source of truth once a gizmo provider is
    registered; without one (upstream FreeCAD, or a build without the GUI
    facade) the request names this module's native backend.
    """
    try:
        import FerroCADGui
    except ImportError:  # upstream FreeCAD has no FerroCAD GUI facade
        return BACKEND
    active_provider = getattr(FerroCADGui, "activeProvider", None)
    if active_provider is None:
        return BACKEND
    return active_provider(FEATURE) or BACKEND


class _GizmoSettings:
    """The ``FerroCAD.Gizmo`` singleton (see the module docstring)."""

    def schema(self) -> list:
        """A copy of :data:`SETTINGS_SCHEMA` (the rows the provider advertises)."""
        return [dict(entry) for entry in SETTINGS_SCHEMA]

    # -- visibility ---------------------------------------------------------
    @property
    def enabled(self) -> bool:
        """Whether the manipulator gizmos are shown."""
        return bool(_read(_SPECS["enabled"]))

    @enabled.setter
    def enabled(self, value) -> None:
        _write(_SPECS["enabled"], bool(value))

    # -- ink ----------------------------------------------------------------
    @property
    def active_color(self) -> str:
        """The hovered/active handle's ink as a canonical lowercase ``#rrggbb``."""
        return _colour(_read(_SPECS["active_color"]))

    @active_color.setter
    def active_color(self, value) -> None:
        _write(_SPECS["active_color"], _colour(value))

    @property
    def inactive_color(self) -> str:
        """The idle handle's ink as a canonical lowercase ``#rrggbb``."""
        return _colour(_read(_SPECS["inactive_color"]))

    @inactive_color.setter
    def inactive_color(self, value) -> None:
        _write(_SPECS["inactive_color"], _colour(value))

    @property
    def outline_color(self) -> str:
        """The outline/halo ink as a canonical lowercase ``#rrggbb``.

        The halo is the non-text-contrast aid (WCAG 1.4.11); it separates a handle
        from mid-luminance geometry without relying on hue.
        """
        return _colour(_read(_SPECS["outline_color"]))

    @outline_color.setter
    def outline_color(self, value) -> None:
        _write(_SPECS["outline_color"], _colour(value))

    # -- geometry -----------------------------------------------------------
    @property
    def size(self) -> float:
        """The handle size in pixels."""
        return _measured(_SPECS["size"], _read(_SPECS["size"]), "the gizmo size")

    @size.setter
    def size(self, value) -> None:
        _write(_SPECS["size"], _measured(_SPECS["size"], value, "the gizmo size"))

    @property
    def width(self) -> float:
        """The handle stroke width in pixels."""
        return _measured(_SPECS["width"], _read(_SPECS["width"]), "the gizmo width")

    @width.setter
    def width(self, value) -> None:
        _write(_SPECS["width"], _measured(_SPECS["width"], value, "the gizmo width"))

    @property
    def halo_width(self) -> float:
        """The outline width around a handle, in pixels; ``0`` disables it."""
        return _measured(
            _SPECS["halo_width"],
            _read(_SPECS["halo_width"]),
            "the gizmo halo width",
        )

    @halo_width.setter
    def halo_width(self, value) -> None:
        _write(
            _SPECS["halo_width"],
            _measured(_SPECS["halo_width"], value, "the gizmo halo width"),
        )

    @property
    def hover_scale(self) -> float:
        """The stroke-width multiplier of the hovered handle (``1`` disables)."""
        return _measured(
            _SPECS["hover_scale"],
            _read(_SPECS["hover_scale"]),
            "the gizmo hover scale",
        )

    @hover_scale.setter
    def hover_scale(self, value) -> None:
        _write(
            _SPECS["hover_scale"],
            _measured(_SPECS["hover_scale"], value, "the gizmo hover scale"),
        )

    @property
    def hit_radius_px(self) -> float:
        """The analytic grab radius in pixels (at least 12, for a 24 px target)."""
        return _measured(
            _SPECS["hit_radius_px"],
            _read(_SPECS["hit_radius_px"]),
            "the gizmo hit radius",
        )

    @hit_radius_px.setter
    def hit_radius_px(self, value) -> None:
        _write(
            _SPECS["hit_radius_px"],
            _measured(_SPECS["hit_radius_px"], value, "the gizmo hit radius"),
        )

    @property
    def opacity(self) -> float:
        """The handles' opacity in ``0..1``."""
        return _measured(
            _SPECS["opacity"],
            _read(_SPECS["opacity"]),
            "the gizmo opacity",
        )

    @opacity.setter
    def opacity(self, value) -> None:
        _write(
            _SPECS["opacity"],
            _measured(_SPECS["opacity"], value, "the gizmo opacity"),
        )

    @property
    def ghost_alpha(self) -> float:
        """The alpha of the occluded (ghosted) handles in ``0..0.5``.

        The ceiling keeps an occluded handle visibly *behind* a front one; a value
        above half reads as another handle rather than an occluded one. A legacy
        stored value above the ceiling is reported clamped so the API never breaks.
        """
        value = _number(
            _read(_SPECS["ghost_alpha"]),
            "the gizmo ghost alpha",
            minimum=0.0,
            maximum=1.0,
        )
        return min(value, MAX_GHOST_ALPHA)

    @ghost_alpha.setter
    def ghost_alpha(self, value) -> None:
        number = _number(value, "the gizmo ghost alpha", minimum=0.0, maximum=1.0)
        _write(_SPECS["ghost_alpha"], min(number, MAX_GHOST_ALPHA))

    @property
    def ring_snap_deg(self) -> float:
        """The rotation ring's snap increment in degrees (a 5° multiple; 0 = free)."""
        return _ring_snap(_read(_SPECS["ring_snap_deg"]))

    @ring_snap_deg.setter
    def ring_snap_deg(self, value) -> None:
        _write(_SPECS["ring_snap_deg"], _ring_snap(value))

    # -- decoration ---------------------------------------------------------
    @property
    def show_readout(self) -> bool:
        """Whether the drag readout is drawn next to the gizmo."""
        return bool(_read(_SPECS["show_readout"]))

    @show_readout.setter
    def show_readout(self, value) -> None:
        _write(_SPECS["show_readout"], bool(value))

    # -- applying -----------------------------------------------------------
    def apply(self) -> dict:
        """The ``{"view": …}`` request for the current settings."""
        return {
            "view": {
                "gizmo": {
                    "enabled": self.enabled,
                    "backend": _backend(),
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

    def snapshot(self) -> dict:
        """The stored gizmo policy, in the same shape as the ``gizmo`` view.

        ``backend`` is not a setting, so it is not part of the policy; the
        provider registry supplies it when a request is built. Headless (or
        before a shell mirrors anything) this is read back from the parameter
        store.
        """
        return {
            "enabled": self.enabled,
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

    def reset(self) -> None:
        """Clear the gizmo store (only the gizmo provider's own group)."""
        _prefs().Clear()


_INSTANCE = None


def gizmo() -> _GizmoSettings:
    """The ``FerroCAD.Gizmo`` singleton, created on first use."""
    global _INSTANCE
    if _INSTANCE is None:
        _INSTANCE = _GizmoSettings()
    return _INSTANCE
