"""The semantic grid API, exposed as ``FreeCAD.Grid``.

This module is private: the public form is ``FreeCAD.Grid`` (resolved lazily by
``FreeCAD.__getattr__``), not ``import FreeCAD.Grid``. The settings live in
``FreeCAD.ParamGet("User parameter:BaseApp/Preferences/View")`` under the same
keys ``App_ReloadGridSettings`` reads; the facade keeps no state of its own.

``apply()`` builds the ``{"view": …}`` request a command returns. The shell owns
the viewport and the view state, so a property write changes the *desired*
policy, and applying it (through a command) is what repaints — the same contract
as writing a parameter and running ``App_ReloadGridSettings``. Headless
(``FreeCAD.GuiUp == 0``) every setter still validates and stores, and
``apply()`` returns the request without raising.

``opacity`` is a **multiplier** over the visual language's token alphas, not an
absolute alpha: the world grid's minor and major levels stay at 0.15, and a
workplane grid keeps its 1:2 minor:major ratio.

The store is in memory today (``FreeCAD.ParamGet`` is a shim); cross-session
persistence is a Base/parameter-backend gap, not something this API invents.
"""

from __future__ import annotations

import math

import FreeCAD

#: The parameter group the view preferences live in.
PREFERENCES = "User parameter:BaseApp/Preferences/View"

DEFAULT_ENABLED = False
DEFAULT_COLOR = "#94a3b8"
DEFAULT_OPACITY = 1.0
DEFAULT_MINOR = 10.0
DEFAULT_MAJOR = 100.0
DEFAULT_RADIUS = 0.0
DEFAULT_AXIS_CROSS = True
DEFAULT_SHADOWS = False
DEFAULT_SHADOW_OPACITY = 0.30

_HEX_DIGITS = "0123456789abcdefABCDEF"


def _prefs():
    """A fresh handle on the view preference group."""
    return FreeCAD.ParamGet(PREFERENCES)


def _colour(value) -> str:
    """Validate a colour and return its canonical lowercase ``#rrggbb``."""
    if not isinstance(value, str):
        raise ValueError("a grid colour must be a '#rrggbb' string, got %r" % (value,))
    text = value.strip()
    if not text.startswith("#"):
        raise ValueError("a grid colour must start with '#', got %r" % (text,))
    digits = text[1:]
    if len(digits) == 3:
        digits = "".join(digit * 2 for digit in digits)
    elif len(digits) != 6:
        raise ValueError("a grid colour must be #rgb or #rrggbb, got %r" % (text,))
    if any(digit not in _HEX_DIGITS for digit in digits):
        raise ValueError("a grid colour has a non-hex digit, got %r" % (text,))
    return "#" + digits.lower()


def _number(value, name: str, *, minimum=0.0, maximum=None, positive=False) -> float:
    """Validate a finite number, never clamping it."""
    try:
        number = float(value)
    except (TypeError, ValueError):
        raise ValueError("%s must be a number, got %r" % (name, value)) from None
    if not math.isfinite(number):
        raise ValueError("%s must be finite, got %r" % (name, value))
    if positive:
        if number <= 0.0:
            raise ValueError("%s must be positive, got %r" % (name, value))
    elif number < minimum:
        raise ValueError("%s must be at least %s, got %r" % (name, minimum, value))
    if maximum is not None and number > maximum:
        raise ValueError("%s must be at most %s, got %r" % (name, maximum, value))
    return number


def _step(value, name: str) -> float:
    """A grid step: positive and finite."""
    return _number(value, name, positive=True)


def _radius(value) -> float:
    """A grid radius: zero means auto, otherwise positive."""
    return _number(value, "the grid radius", minimum=0.0)


def _alpha(value, name: str) -> float:
    """An opacity: a multiplier in ``0..1``."""
    return _number(value, name, minimum=0.0, maximum=1.0)


class _GridSettings:
    """The ``FreeCAD.Grid`` singleton (see the module docstring)."""

    # -- visibility ---------------------------------------------------------
    @property
    def enabled(self) -> bool:
        """Whether the world grid's ground layer is shown."""
        return _prefs().GetBool("GridEnabled", DEFAULT_ENABLED)

    @enabled.setter
    def enabled(self, value) -> None:
        _prefs().SetBool("GridEnabled", bool(value))

    # -- ink ----------------------------------------------------------------
    @property
    def color(self) -> str:
        """The grid ink as a canonical lowercase ``#rrggbb``."""
        return _colour(_prefs().GetString("GridColor", DEFAULT_COLOR))

    @color.setter
    def color(self, value) -> None:
        _prefs().SetString("GridColor", _colour(value))

    @property
    def opacity(self) -> float:
        """A multiplier (``0..1``) over the token alphas."""
        return _alpha(
            _prefs().GetFloat("GridOpacity", DEFAULT_OPACITY), "the grid opacity"
        )

    @opacity.setter
    def opacity(self, value) -> None:
        _prefs().SetFloat("GridOpacity", _alpha(value, "the grid opacity"))

    # -- steps and extent ---------------------------------------------------
    @property
    def spacing(self) -> float:
        """The minor step (mm); setting it writes the next decade as the major step."""
        return self.minor

    @spacing.setter
    def spacing(self, value) -> None:
        step = _step(value, "the grid spacing")
        prefs = _prefs()
        prefs.SetFloat("GridMinorStep", step)
        prefs.SetFloat("GridMajorStep", step * 10.0)

    @property
    def minor(self) -> float:
        """The world grid's minor (dot) step, in millimetres."""
        return _step(
            _prefs().GetFloat("GridMinorStep", DEFAULT_MINOR),
            "the grid minor step",
        )

    @minor.setter
    def minor(self, value) -> None:
        _prefs().SetFloat("GridMinorStep", _step(value, "the grid minor step"))

    @property
    def major(self) -> float:
        """The world grid's major (line) step, in millimetres."""
        return _step(
            _prefs().GetFloat("GridMajorStep", DEFAULT_MAJOR),
            "the grid major step",
        )

    @major.setter
    def major(self, value) -> None:
        _prefs().SetFloat("GridMajorStep", _step(value, "the grid major step"))

    @property
    def radius(self) -> float:
        """The fixed world-grid extent (mm); ``0`` is camera-derived."""
        return _radius(_prefs().GetFloat("GridRadius", DEFAULT_RADIUS))

    @radius.setter
    def radius(self, value) -> None:
        _prefs().SetFloat("GridRadius", _radius(value))

    # -- orientation --------------------------------------------------------
    @property
    def axis_cross(self) -> bool:
        """Whether the world axis cross (the triad) is shown."""
        return _prefs().GetBool("GridAxisCross", DEFAULT_AXIS_CROSS)

    @axis_cross.setter
    def axis_cross(self, value) -> None:
        _prefs().SetBool("GridAxisCross", bool(value))

    # -- shadows (persisted; no producer yet) -------------------------------
    @property
    def shadows(self) -> bool:
        """Whether grid shadows are requested (nothing draws them yet)."""
        return _prefs().GetBool("GridShadow", DEFAULT_SHADOWS)

    @shadows.setter
    def shadows(self, value) -> None:
        _prefs().SetBool("GridShadow", bool(value))

    @property
    def shadow_opacity(self) -> float:
        """The requested shadow strength, ``0..1`` (nothing draws it yet)."""
        return _alpha(
            _prefs().GetFloat("GridShadowOpacity", DEFAULT_SHADOW_OPACITY),
            "the grid shadow opacity",
        )

    @shadow_opacity.setter
    def shadow_opacity(self, value) -> None:
        _prefs().SetFloat(
            "GridShadowOpacity", _alpha(value, "the grid shadow opacity")
        )

    # -- applying -----------------------------------------------------------
    def apply(self) -> dict:
        """The ``{"view": …}`` request for the current settings.

        Only the keys the shell's ``ViewRequest`` understands are included.
        The shadow preferences persist and appear in :meth:`snapshot`, but
        nothing consumes them yet, so they are not sent.
        """
        minor = self.minor
        major = self.major
        if major < minor:
            raise ValueError(
                "the grid major step (%s) cannot be below the minor one (%s)"
                % (major, minor)
            )
        return {
            "view": {
                "grid": self.enabled,
                "axis_cross": self.axis_cross,
                "grid_minor": minor,
                "grid_major": major,
                "grid_radius": self.radius,
                "grid_color": self.color,
                "grid_alpha": self.opacity,
            }
        }

    def snapshot(self) -> dict:
        """The applied state when the shell mirrors it, else the stored policy.

        With ``FreeCAD.GuiUp`` and a ``FreeCADGui.ViewState``, this is the
        shell's mirror of what is actually drawn; otherwise (headless, or
        before a shell exists) it is read back from the parameter store. A
        mirror that does not carry the colour and alpha fields falls back to
        the store for those two.
        """
        mirror = self._mirror()
        if mirror is not None:
            return {
                "grid": bool(mirror.grid),
                "axis_cross": bool(mirror.axis_cross),
                "grid_minor": float(mirror.grid_minor),
                "grid_major": float(mirror.grid_major),
                "grid_radius": (
                    None if mirror.grid_radius is None else float(mirror.grid_radius)
                ),
                "grid_color": getattr(mirror, "grid_color", self.color),
                "grid_alpha": float(getattr(mirror, "grid_alpha", self.opacity)),
                "shadows": self.shadows,
                "shadow_opacity": self.shadow_opacity,
            }
        return {
            "grid": self.enabled,
            "axis_cross": self.axis_cross,
            "grid_minor": self.minor,
            "grid_major": self.major,
            "grid_radius": self.radius if self.radius > 0.0 else None,
            "grid_color": self.color,
            "grid_alpha": self.opacity,
            "shadows": self.shadows,
            "shadow_opacity": self.shadow_opacity,
        }

    @staticmethod
    def _mirror():
        """``FreeCADGui.ViewState`` when a GUI is up and exposes it, else ``None``."""
        if not FreeCAD.GuiUp:
            return None
        try:
            import FreeCADGui
        except Exception:  # noqa: BLE001 - no GUI module means no mirror
            return None
        return getattr(FreeCADGui, "ViewState", None)

    def reset(self) -> None:
        """Clear every grid preference in the view group (back to the defaults)."""
        _prefs().Clear()


_INSTANCE = None


def grid() -> _GridSettings:
    """The ``FreeCAD.Grid`` singleton, created on first use."""
    global _INSTANCE
    if _INSTANCE is None:
        _INSTANCE = _GridSettings()
    return _INSTANCE
