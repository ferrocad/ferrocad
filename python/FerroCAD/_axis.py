"""The semantic axis-cross API, exposed as ``FerroCAD.Axis``.

This module is private: the public form is ``FerroCAD.Axis`` (the singleton the
package binds lazily), not ``import FerroCAD._axis``. The settings live in the
native axis provider's parameter group,
``FreeCAD.ParamGet("User parameter:BaseApp/Preferences/Mods/axis/ferrocad")``,
under one typed key per :data:`SETTINGS_SCHEMA` entry, so the attribute API and
``Settings/Axis`` are two views of the same store. The facade keeps no state of
its own.

The axis cross used to be a boolean in the grid capability
(``FerroCAD.Grid.axis_cross``). That flag moved here with the rest of the axis
look, and ``FerroCAD.Grid.axis_cross`` remains as a deprecated proxy. The first
read of an unset ``enabled`` migrates the old ``GridAxisCross`` value once — from
the native grid provider's group, else from the pre-S4 ``Preferences/View``
group — so a user's existing choice survives the move. Only ``enabled`` is
migrated; every other key is new and is never written by a read.

``apply()`` builds the ``{"view": …}`` request a command returns. The shell owns
the viewport, so a property write changes the *desired* policy, and applying it
(through a command) is what repaints. Headless (``FreeCAD.GuiUp == 0``) every
setter still validates and stores, and ``apply()`` returns the request without
raising.
"""

from __future__ import annotations

import math

import FreeCAD

#: The provider feature and backend whose parameter group is the axis store.
FEATURE = "axis"
BACKEND = "ferrocad"

#: The parameter group the axis values live in (the native provider's group).
PREFERENCES = "User parameter:BaseApp/Preferences/Mods/%s/%s" % (FEATURE, BACKEND)

#: The native grid provider's group, where the axis flag lived as ``GridAxisCross``.
LEGACY_GRID_PREFERENCES = "User parameter:BaseApp/Preferences/Mods/grid/ferrocad"

#: The pre-S4 view preference group, the older home of ``GridAxisCross``.
LEGACY_PREFERENCES = "User parameter:BaseApp/Preferences/View"

DEFAULT_ENABLED = True
DEFAULT_COLOR_X = "#f87171"
DEFAULT_COLOR_Y = "#4ade80"
DEFAULT_COLOR_Z = "#38bdf8"
DEFAULT_OPACITY = 0.65
DEFAULT_LENGTH = 80.0
DEFAULT_WIDTH = 1.5
DEFAULT_ORIGIN = True
DEFAULT_LABELS = True
DEFAULT_GHOST_ALPHA = 0.10

#: The canonical axis schema. It is the one description of the axis cross's
#: knobs: ``FerroCADGui.registerProvider(..., settings=SETTINGS_SCHEMA)``
#: advertises these rows, and :class:`_AxisSettings` stores each attribute under
#: the entry's ``name`` in :data:`PREFERENCES`. The kinds are the ones the
#: property editor understands.
SETTINGS_SCHEMA = (
    {
        "name": "enabled",
        "label": "Show axis cross",
        "kind": "bool",
        "default": DEFAULT_ENABLED,
    },
    {
        "name": "color_x",
        "label": "X axis colour",
        "kind": "color",
        "default": DEFAULT_COLOR_X,
    },
    {
        "name": "color_y",
        "label": "Y axis colour",
        "kind": "color",
        "default": DEFAULT_COLOR_Y,
    },
    {
        "name": "color_z",
        "label": "Z axis colour",
        "kind": "color",
        "default": DEFAULT_COLOR_Z,
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
        "name": "length",
        "label": "Stem length (px)",
        "kind": "float",
        "default": DEFAULT_LENGTH,
        "min": 8.0,
        "max": 400.0,
    },
    {
        "name": "width",
        "label": "Stem width (px)",
        "kind": "float",
        "default": DEFAULT_WIDTH,
        "min": 0.5,
        "max": 6.0,
    },
    {
        "name": "origin",
        "label": "Origin node",
        "kind": "bool",
        "default": DEFAULT_ORIGIN,
    },
    {
        "name": "labels",
        "label": "Axis labels",
        "kind": "bool",
        "default": DEFAULT_LABELS,
    },
    {
        "name": "ghost_alpha",
        "label": "Occluded alpha",
        "kind": "float",
        "default": DEFAULT_GHOST_ALPHA,
        "min": 0.0,
        "max": 1.0,
    },
)

#: The same schema under the explicit name the axis provider advertises.
AXIS_SETTINGS_SCHEMA = SETTINGS_SCHEMA

_SPECS = {entry["name"]: entry for entry in SETTINGS_SCHEMA}

_HEX_DIGITS = "0123456789abcdefABCDEF"


def _prefs():
    """A fresh handle on the axis provider's parameter group."""
    return FreeCAD.ParamGet(PREFERENCES)


def _legacy_grid_prefs():
    """A fresh handle on the native grid provider's parameter group."""
    return FreeCAD.ParamGet(LEGACY_GRID_PREFERENCES)


def _legacy_view_prefs():
    """A fresh handle on the pre-S4 view preference group."""
    return FreeCAD.ParamGet(LEGACY_PREFERENCES)


def _present(group, name: str, kind: str) -> bool:
    """Whether ``group`` holds a value of ``kind`` called ``name``.

    A typed parameter group cannot distinguish "false" from "unset" through
    ``GetBool``, so migration asks the name lists instead.
    """
    if kind == "bool":
        return name in group.GetBools()
    if kind in ("float", "quantity"):
        return name in group.GetFloats()
    return name in group.GetStrings()


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


def _migrate_enabled(group) -> None:
    """Copy the legacy ``GridAxisCross`` flag into ``enabled`` once.

    The axis group's ``enabled`` key is the marker: once it exists the axis store
    is authoritative. The grid provider's group is consulted first (the key's
    most recent old home), then the pre-S4 ``Preferences/View`` group. Nothing
    else is written, and no other key is read.
    """
    spec = _SPECS["enabled"]
    if _present(group, "enabled", "bool"):
        return
    value = DEFAULT_ENABLED
    for legacy in (_legacy_grid_prefs(), _legacy_view_prefs()):
        if _present(legacy, "GridAxisCross", "bool"):
            value = legacy.GetBool("GridAxisCross", DEFAULT_ENABLED)
            break
    _store(group, spec, value)


def _read(spec: dict):
    """One schema value, migrating the legacy axis flag first when necessary."""
    group = _prefs()
    if spec["name"] == "enabled":
        _migrate_enabled(group)
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
        raise ValueError("an axis colour must be a '#rrggbb' string, got %r" % (value,))
    text = value.strip()
    if not text.startswith("#"):
        raise ValueError("an axis colour must start with '#', got %r" % (text,))
    digits = text[1:]
    if len(digits) == 3:
        digits = "".join(digit * 2 for digit in digits)
    elif len(digits) != 6:
        raise ValueError("an axis colour must be #rgb or #rrggbb, got %r" % (text,))
    if any(digit not in _HEX_DIGITS for digit in digits):
        raise ValueError("an axis colour has a non-hex digit, got %r" % (text,))
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


class _AxisSettings:
    """The ``FerroCAD.Axis`` singleton (see the module docstring)."""

    def schema(self) -> list:
        """A copy of :data:`SETTINGS_SCHEMA` (the rows the provider advertises)."""
        return [dict(entry) for entry in SETTINGS_SCHEMA]

    # -- visibility ---------------------------------------------------------
    @property
    def enabled(self) -> bool:
        """Whether the world axis cross is shown."""
        return bool(_read(_SPECS["enabled"]))

    @enabled.setter
    def enabled(self, value) -> None:
        _write(_SPECS["enabled"], bool(value))

    # -- ink ----------------------------------------------------------------
    @property
    def color_x(self) -> str:
        """The X stem's ink as a canonical lowercase ``#rrggbb``."""
        return _colour(_read(_SPECS["color_x"]))

    @color_x.setter
    def color_x(self, value) -> None:
        _write(_SPECS["color_x"], _colour(value))

    @property
    def color_y(self) -> str:
        """The Y stem's ink as a canonical lowercase ``#rrggbb``."""
        return _colour(_read(_SPECS["color_y"]))

    @color_y.setter
    def color_y(self, value) -> None:
        _write(_SPECS["color_y"], _colour(value))

    @property
    def color_z(self) -> str:
        """The Z stem's ink as a canonical lowercase ``#rrggbb``."""
        return _colour(_read(_SPECS["color_z"]))

    @color_z.setter
    def color_z(self, value) -> None:
        _write(_SPECS["color_z"], _colour(value))

    @property
    def opacity(self) -> float:
        """The stems' opacity in ``0..1``."""
        return _measured(_SPECS["opacity"], _read(_SPECS["opacity"]), "the axis opacity")

    @opacity.setter
    def opacity(self, value) -> None:
        _write(
            _SPECS["opacity"],
            _measured(_SPECS["opacity"], value, "the axis opacity"),
        )

    # -- geometry -----------------------------------------------------------
    @property
    def length(self) -> float:
        """The stem length in pixels."""
        return _measured(_SPECS["length"], _read(_SPECS["length"]), "the axis length")

    @length.setter
    def length(self, value) -> None:
        _write(
            _SPECS["length"],
            _measured(_SPECS["length"], value, "the axis length"),
        )

    @property
    def width(self) -> float:
        """The stem width in pixels."""
        return _measured(_SPECS["width"], _read(_SPECS["width"]), "the axis width")

    @width.setter
    def width(self, value) -> None:
        _write(
            _SPECS["width"],
            _measured(_SPECS["width"], value, "the axis width"),
        )

    # -- decoration ---------------------------------------------------------
    @property
    def origin(self) -> bool:
        """Whether the origin node is drawn at the cross's centre."""
        return bool(_read(_SPECS["origin"]))

    @origin.setter
    def origin(self, value) -> None:
        _write(_SPECS["origin"], bool(value))

    @property
    def labels(self) -> bool:
        """Whether the ``X``/``Y``/``Z`` labels are drawn."""
        return bool(_read(_SPECS["labels"]))

    @labels.setter
    def labels(self, value) -> None:
        _write(_SPECS["labels"], bool(value))

    @property
    def ghost_alpha(self) -> float:
        """The alpha of the occluded (ghosted) stems in ``0..1``."""
        return _measured(
            _SPECS["ghost_alpha"],
            _read(_SPECS["ghost_alpha"]),
            "the axis ghost alpha",
        )

    @ghost_alpha.setter
    def ghost_alpha(self, value) -> None:
        _write(
            _SPECS["ghost_alpha"],
            _measured(_SPECS["ghost_alpha"], value, "the axis ghost alpha"),
        )

    # -- applying -----------------------------------------------------------
    def apply(self) -> dict:
        """The ``{"view": …}`` request for the current settings."""
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

    def snapshot(self) -> dict:
        """The stored axis policy, in the same shape as the ``axis`` view.

        Headless (or before a shell mirrors anything) this is read back from the
        parameter store. The shell owns the applied mirror; the policy is the
        facade's own state.
        """
        return {
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

    def reset(self) -> None:
        """Clear the axis store (only the axis provider's own group).

        The legacy ``GridAxisCross`` keys belong to other groups and are left
        alone: this facade never removes preferences it does not own.
        """
        _prefs().Clear()


_INSTANCE = None


def axis() -> _AxisSettings:
    """The ``FerroCAD.Axis`` singleton, created on first use."""
    global _INSTANCE
    if _INSTANCE is None:
        _INSTANCE = _AxisSettings()
    return _INSTANCE
