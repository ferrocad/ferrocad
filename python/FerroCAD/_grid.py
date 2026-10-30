"""The semantic grid API, exposed as ``FerroCAD.Grid``.

This module is private: the public form is ``FerroCAD.Grid`` (the singleton the
package binds on import), not ``import FerroCAD._grid``. The settings live in the
native grid provider's parameter group,
``FreeCAD.ParamGet("User parameter:BaseApp/Preferences/Mods/grid/ferrocad")``,
under one typed key per :data:`SETTINGS_SCHEMA` entry, so the attribute API and
``Settings/Grid`` are two views of the same store. The facade keeps no state of
its own.

The legacy layout (``User parameter:BaseApp/Preferences/View`` with ``Grid*``
keys) is read once: the first read of an unset key copies the legacy value into
the provider group, so a user's existing settings survive the move. ``reset()``
clears both, which keeps tests and a deliberate "back to defaults" clean.

``apply()`` builds the ``{"view": …}`` request a command returns. The shell owns
the viewport and the view state, so a property write changes the *desired*
policy, and applying it (through a command) is what repaints — the same contract
as writing a parameter and running ``App_ReloadGridSettings``. Headless
(``FreeCAD.GuiUp == 0``) every setter still validates and stores, and
``apply()`` returns the request without raising.

``opacity`` is a **multiplier** over the visual language's token alphas, not an
absolute alpha: the world grid's minor and major levels stay at 0.15, and a
workplane grid keeps its 1:2 minor:major ratio.

``minor`` and ``major`` are the **configured** levels. Spacing is ``adaptive`` by
default, and then the step follows the extent the layer covers instead: the ground
disc's radius for the world ground, a face's in-plane radius for a bound plane, on
the 1-2-5 ladder. Clearing ``adaptive`` draws the configured pair on every plane,
so the same two numbers describe the whole viewport.

The levels are always a **subdivision** apart: ``major`` is strictly coarser than
``minor``. A minor step equal to the major one has no rows of its own (every row at
that step is a major row), so the minor level would vanish and ``minor_style`` and
``minor_width`` would silently do nothing; writing the same number into the major
row therefore lowers the minor by a decade, the relation ``spacing`` uses.

The store is in memory until the parameter backend is file-backed; cross-session
persistence is a Base/parameter-backend concern, not something this API invents.
"""

from __future__ import annotations

import math

import FreeCAD

#: The provider feature and backend whose parameter group is the grid store.
FEATURE = "grid"
BACKEND = "ferrocad"

#: The parameter group the grid values live in (the native provider's group).
PREFERENCES = "User parameter:BaseApp/Preferences/Mods/%s/%s" % (FEATURE, BACKEND)

#: The pre-S4 parameter group, read once to migrate a user's old values.
LEGACY_PREFERENCES = "User parameter:BaseApp/Preferences/View"

DEFAULT_ENABLED = False
DEFAULT_COLOR = "#94a3b8"
DEFAULT_OPACITY = 1.0
DEFAULT_ADAPTIVE = True
DEFAULT_MINOR_STYLE = "dots"
DEFAULT_MAJOR_WIDTH = 1.0
DEFAULT_MINOR_WIDTH = 1.0
DEFAULT_MINOR = 10.0
DEFAULT_MAJOR = 100.0
DEFAULT_RADIUS = 0.0
DEFAULT_SHADOWS = False
DEFAULT_SHADOW_OPACITY = 0.30

def _spacing_status() -> str:
    """The property pane's chip for ``minor``/``major``.

    While ``adaptive`` is on the drawn step is derived from the extent the layer
    covers, so these two rows are the **fallback**, not what is on screen. The chip
    says so (``10 mm  [adaptive]``) instead of showing a number that silently does
    nothing when it is edited.
    """
    return "adaptive" if grid().adaptive else "provider"


#: The canonical grid schema. It is the one description of the grid's knobs:
#: ``FerroCADGui.registerProvider(..., settings=SETTINGS_SCHEMA)`` advertises
#: these rows, and :class:`_GridSettings` stores each attribute under the entry's
#: ``name`` in :data:`PREFERENCES`. The kinds are the ones the property editor
#: understands; a ``quantity`` is stored as a float in its declared unit.
#:
#: ``spacing`` is deliberately absent: it is a convenience alias over ``minor``
#: (setting it writes the next decade as ``major``), not a separate knob.
#:
#: A ``status`` may be a string or a callable; the property pane shows it as a chip
#: beside the value. The spacing rows use it to say they are ``adaptive``.
SETTINGS_SCHEMA = (
    {
        "name": "enabled",
        "label": "Show grid",
        "kind": "bool",
        "default": DEFAULT_ENABLED,
    },
    {
        "name": "color",
        "label": "Grid colour",
        "kind": "color",
        "default": DEFAULT_COLOR,
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
        "name": "adaptive",
        "label": "Adaptive spacing",
        "kind": "bool",
        "default": DEFAULT_ADAPTIVE,
    },
    {
        "name": "minor_style",
        "label": "Minor grid style",
        "kind": "enum",
        "choices": ["dots", "lines"],
        "default": DEFAULT_MINOR_STYLE,
    },
    {
        "name": "major_width",
        "label": "Line width (px)",
        "kind": "float",
        "default": DEFAULT_MAJOR_WIDTH,
        "min": 1.0,
        "max": 8.0,
    },
    {
        "name": "minor_width",
        "label": "Minor line width (px)",
        "kind": "float",
        "default": DEFAULT_MINOR_WIDTH,
        "min": 1.0,
        "max": 8.0,
    },
    {
        "name": "minor",
        "label": "Minor spacing",
        "kind": "quantity",
        "unit": "mm",
        "default": DEFAULT_MINOR,
        "min": 0.001,
        "status": _spacing_status,
    },
    {
        "name": "major",
        "label": "Major spacing",
        "kind": "quantity",
        "unit": "mm",
        "default": DEFAULT_MAJOR,
        "min": 0.001,
        "status": _spacing_status,
    },
    {
        "name": "radius",
        "label": "Grid extent",
        "kind": "quantity",
        "unit": "mm",
        "default": DEFAULT_RADIUS,
        "min": 0.0,
    },
    {
        "name": "shadows",
        "label": "Show ground shadow",
        "kind": "bool",
        "default": DEFAULT_SHADOWS,
    },
    {
        "name": "shadow_opacity",
        "label": "Shadow opacity",
        "kind": "float",
        "default": DEFAULT_SHADOW_OPACITY,
        "min": 0.0,
        "max": 1.0,
    },
)

#: Schema name -> the key the same value had before S4.
_LEGACY_KEYS = {
    "enabled": "GridEnabled",
    "color": "GridColor",
    "opacity": "GridOpacity",
    "minor": "GridMinorStep",
    "major": "GridMajorStep",
    "radius": "GridRadius",
    "shadows": "GridShadow",
    "shadow_opacity": "GridShadowOpacity",
}

_SPECS = {entry["name"]: entry for entry in SETTINGS_SCHEMA}

_HEX_DIGITS = "0123456789abcdefABCDEF"


def _prefs():
    """A fresh handle on the grid provider's parameter group."""
    return FreeCAD.ParamGet(PREFERENCES)


def _legacy_prefs():
    """A fresh handle on the pre-S4 view preference group."""
    return FreeCAD.ParamGet(LEGACY_PREFERENCES)


def _present(group, name: str, kind: str) -> bool:
    """Whether ``group`` holds a value of ``kind`` called ``name``.

    A typed parameter group cannot distinguish "false" from "unset" through
    ``GetBool``, so migration and reset ask the name lists instead.
    """
    if kind == "bool":
        return name in group.GetBools()
    if kind in ("float", "quantity"):
        return name in group.GetFloats()
    return name in group.GetStrings()


def _read_group(group, name: str, kind: str):
    """Read one typed value with a neutral fallback (the caller applies defaults)."""
    if kind == "bool":
        return group.GetBool(name, False)
    if kind in ("float", "quantity"):
        return group.GetFloat(name, 0.0)
    return group.GetString(name, "")


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


def _migrate(spec: dict, group) -> None:
    """Copy a legacy value into the provider group, once per key.

    The new key's presence is the marker: once migrated, the provider group is
    authoritative and the legacy key is ignored (it is removed by ``reset()``).
    """
    legacy_key = _LEGACY_KEYS.get(spec["name"])
    if legacy_key is None or _present(group, spec["name"], spec["kind"]):
        return
    legacy = _legacy_prefs()
    if not _present(legacy, legacy_key, spec["kind"]):
        return
    _store(group, spec, _read_group(legacy, legacy_key, spec["kind"]))


def _read(spec: dict):
    """One schema value, migrating a legacy value first when necessary."""
    group = _prefs()
    _migrate(spec, group)
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


def _style(value, name: str) -> str:
    """A minor-level form: ``"dots"`` or ``"lines"``."""
    if not isinstance(value, str):
        raise ValueError("%s must be \"dots\" or \"lines\", got %r" % (name, value))
    text = value.strip().lower()
    if text not in ("dots", "lines"):
        raise ValueError("%s must be \"dots\" or \"lines\", got %r" % (name, value))
    return text


def _width(value, name: str) -> float:
    """A stroke width in device pixels, bounded as the viewport clamps it."""
    return _number(value, name, minimum=1.0, maximum=8.0)


class _GridSettings:
    """The ``FerroCAD.Grid`` singleton (see the module docstring)."""

    def schema(self) -> list:
        """A copy of :data:`SETTINGS_SCHEMA` (the rows the provider advertises)."""
        return [dict(entry) for entry in SETTINGS_SCHEMA]

    # -- visibility ---------------------------------------------------------
    @property
    def enabled(self) -> bool:
        """Whether the world grid's ground layer is shown."""
        return bool(_read(_SPECS["enabled"]))

    @enabled.setter
    def enabled(self, value) -> None:
        _write(_SPECS["enabled"], bool(value))

    # -- ink ----------------------------------------------------------------
    @property
    def color(self) -> str:
        """The grid ink as a canonical lowercase ``#rrggbb``."""
        return _colour(_read(_SPECS["color"]))

    @color.setter
    def color(self, value) -> None:
        _write(_SPECS["color"], _colour(value))

    @property
    def opacity(self) -> float:
        """A multiplier (``0..1``) over the token alphas."""
        return _alpha(_read(_SPECS["opacity"]), "the grid opacity")

    @opacity.setter
    def opacity(self, value) -> None:
        _write(_SPECS["opacity"], _alpha(value, "the grid opacity"))

    # -- steps and extent ---------------------------------------------------
    @property
    def adaptive(self) -> bool:
        """Whether the grid's step follows the extent the layer covers.

        On (the default), the step is derived from what the layer spans — the ground
        disc's radius for the world ground, a face's in-plane radius for a bound plane —
        on the 1-2-5 ladder, so a small part and a large one both get about ten major
        cells. Off, `minor` and `major` are drawn on every plane and the extent only
        sizes the patch.
        """
        return bool(_read(_SPECS["adaptive"]))

    @adaptive.setter
    def adaptive(self, value) -> None:
        _write(_SPECS["adaptive"], bool(value))

    @property
    def minor_style(self) -> str:
        """The minor level's form: ``"dots"`` (intersection dots) or ``"lines"``.

        Both grid backends draw the same form: dots are the product of the two axes'
        coverage, and ``lines`` is a full lattice at the minor step (the graph-paper
        look). The major lattice is always lines.
        """
        return _style(_read(_SPECS["minor_style"]), "the minor grid style")

    @minor_style.setter
    def minor_style(self, value) -> None:
        _write(_SPECS["minor_style"], _style(value, "the minor grid style"))

    @property
    def major_width(self) -> float:
        """The major lines' stroke width, in device pixels (``1`` is the hairline)."""
        return _width(_read(_SPECS["major_width"]), "the major line width")

    @major_width.setter
    def major_width(self, value) -> None:
        _write(_SPECS["major_width"], _width(value, "the major line width"))

    @property
    def minor_width(self) -> float:
        """The minor level's stroke width, in device pixels.

        It is the minor lattice's line width when `minor_style` is ``"lines"``, and the
        dot's diameter when it is ``"dots"`` — a dot is an in-plane square of that size,
        the same square the Coin shader's two-axis coverage draws. ``1`` is the
        renderer's hairline/1 px point.
        """
        return _width(_read(_SPECS["minor_width"]), "the minor line width")

    @minor_width.setter
    def minor_width(self, value) -> None:
        _write(_SPECS["minor_width"], _width(value, "the minor line width"))

    @property
    def spacing(self) -> float:
        """The minor step (mm); setting it writes the next decade as the major step."""
        return self.minor

    @spacing.setter
    def spacing(self, value) -> None:
        step = _step(value, "the grid spacing")
        _write(_SPECS["minor"], step)
        _write(_SPECS["major"], step * 10.0)

    @property
    def minor(self) -> float:
        """The world grid's minor (dot) step, in millimetres.

        With `adaptive` on this is the configured level, not necessarily the drawn one:
        the step is resolved from the extent each layer covers.
        """
        return _step(_read(_SPECS["minor"]), "the grid minor step")

    @minor.setter
    def minor(self, value) -> None:
        step = _step(value, "the grid minor step")
        _write(_SPECS["minor"], step)
        # Keep the decade ordering: `apply()` requires `major > minor`, so a bare
        # "bump the minor" would make the whole grid unrequestable.
        if step >= self.major:
            _write(_SPECS["major"], step * 10.0)

    @property
    def major(self) -> float:
        """The world grid's major (line) step, in millimetres."""
        return _step(_read(_SPECS["major"]), "the grid major step")

    @major.setter
    def major(self, value) -> None:
        step = _step(value, "the grid major step")
        minor = self.minor
        if step < minor:
            raise ValueError(
                "the grid major step (%s) cannot be below the minor one (%s)"
                % (step, minor)
            )
        _write(_SPECS["major"], step)
        if step == minor:
            # The two levels must be a subdivision apart. A minor step equal to the
            # major one has no rows of its own — `grid::ground` skips every row that is
            # a major row — so the minor level vanishes and `minor_style` / `minor_width`
            # silently do nothing. Lower the minor by a decade, the relation `spacing`
            # writes, so typing `10 mm` into the major row means "a 10 mm grid with
            # 1 mm subdivisions" rather than "one level, styled by the major only".
            _write(_SPECS["minor"], step / 10.0)

    @property
    def radius(self) -> float:
        """The fixed world-grid extent (mm); ``0`` is camera-derived."""
        return _radius(_read(_SPECS["radius"]))

    @radius.setter
    def radius(self, value) -> None:
        _write(_SPECS["radius"], _radius(value))

    # -- orientation (deprecated alias of FerroCAD.Axis) ---------------------
    @property
    def axis_cross(self) -> bool:
        """Deprecated: moved to :attr:`FerroCAD.Axis.enabled`.

        The axis cross is its own capability now; this proxy keeps older mods
        and scripts working. New code should use ``FerroCAD.Axis``.
        """
        from . import _axis

        return _axis.axis().enabled

    @axis_cross.setter
    def axis_cross(self, value) -> None:
        from . import _axis

        _axis.axis().enabled = value

    # -- shadows (persisted; no producer yet) -------------------------------
    @property
    def shadows(self) -> bool:
        """Whether grid shadows are requested (nothing draws them yet)."""
        return bool(_read(_SPECS["shadows"]))

    @shadows.setter
    def shadows(self, value) -> None:
        _write(_SPECS["shadows"], bool(value))

    @property
    def shadow_opacity(self) -> float:
        """The requested shadow strength, ``0..1`` (nothing draws it yet)."""
        return _alpha(_read(_SPECS["shadow_opacity"]), "the grid shadow opacity")

    @shadow_opacity.setter
    def shadow_opacity(self, value) -> None:
        _write(
            _SPECS["shadow_opacity"],
            _alpha(value, "the grid shadow opacity"),
        )

    # -- applying -----------------------------------------------------------
    def apply(self) -> dict:
        """The ``{"view": …}`` request for the current settings.

        Only the keys the shell's ``ViewRequest`` understands are included.
        The shadow preferences persist and appear in :meth:`snapshot`, but
        nothing consumes them yet, so they are not sent.

        The pair is healed rather than refused: a store written behind the setters
        (a legacy key, an external edit) can hold ``major <= minor``, and raising here
        would mean *no* view request at all — the viewport would keep drawing whatever
        it had, which is how a disabled grid stayed on screen. A healed pair is written
        back, so the pane and the drawing agree.
        """
        minor = self.minor
        major = self.major
        if major < minor:
            major = minor * 10.0
            _write(_SPECS["major"], major)
        elif major == minor:
            # Equal levels leave the minor level with nothing to draw (see the
            # `major` setter); split them a decade apart.
            minor = major / 10.0
            _write(_SPECS["minor"], minor)
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

    def snapshot(self) -> dict:
        """The applied state when the shell mirrors it, else the stored policy.

        With ``FreeCAD.GuiUp`` and a ``FerroCADGui.ViewState``, this is the
        shell's mirror of what is actually drawn; otherwise (headless, or
        before a shell exists) it is read back from the parameter store. A
        mirror that does not carry the colour and alpha fields falls back to
        the store for those two.
        """
        mirror = self._mirror()
        if mirror is not None:
            return {
                "grid": bool(mirror.grid),
                "grid_minor": float(mirror.grid_minor),
                "grid_major": float(mirror.grid_major),
                "grid_adaptive": bool(getattr(mirror, "grid_adaptive", self.adaptive)),
                "grid_minor_style": str(
                    getattr(mirror, "grid_minor_style", self.minor_style)
                ),
                "grid_major_width": float(
                    getattr(mirror, "grid_major_width", self.major_width)
                ),
                "grid_minor_width": float(
                    getattr(mirror, "grid_minor_width", self.minor_width)
                ),
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
            "grid_minor": self.minor,
            "grid_major": self.major,
            "grid_adaptive": self.adaptive,
            "grid_minor_style": self.minor_style,
            "grid_major_width": self.major_width,
            "grid_minor_width": self.minor_width,
            "grid_radius": self.radius if self.radius > 0.0 else None,
            "grid_color": self.color,
            "grid_alpha": self.opacity,
            "shadows": self.shadows,
            "shadow_opacity": self.shadow_opacity,
        }

    @staticmethod
    def _mirror():
        """``FerroCADGui.ViewState`` when a GUI is up and exposes it, else ``None``."""
        if not FreeCAD.GuiUp:
            return None
        try:
            import FerroCADGui
        except Exception:  # noqa: BLE001 - no GUI module means no mirror
            return None
        return getattr(FerroCADGui, "ViewState", None)

    def reset(self) -> None:
        """Clear the grid store, including the pre-S4 legacy keys.

        Only the grid's own values are removed: the legacy group also holds
        unrelated view preferences, so it is never cleared wholesale.
        """
        _prefs().Clear()
        legacy = _legacy_prefs()
        for name, key in _LEGACY_KEYS.items():
            kind = _SPECS[name]["kind"]
            if kind == "bool":
                legacy.RemBool(key)
            elif kind in ("float", "quantity"):
                legacy.RemFloat(key)
            else:
                legacy.RemString(key)


_INSTANCE = None


def grid() -> _GridSettings:
    """The ``FerroCAD.Grid`` singleton, created on first use."""
    global _INSTANCE
    if _INSTANCE is None:
        _INSTANCE = _GridSettings()
    return _INSTANCE
