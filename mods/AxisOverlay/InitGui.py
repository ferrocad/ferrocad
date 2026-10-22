# -*- coding: utf-8 -*-
"""Axis Overlay for FerroCAD: settings for the native world axis cross.

The command (``AxisOverlay_Settings``) is the axis cross's one settings surface.
Like ``mods/GridOverlay`` it is a thin client of the semantic API rather than a
scene-graph addon: there is no Qt, no ``pivy`` and no per-document overlay tree.
The values go to ``FerroCAD.Axis`` and the shell draws them with the native axis
cross.

The mod declares itself as the ``"ferrocad"`` backend of the ``"axis"`` feature
(priority 10, so the native axis cross is the default), advertising the canonical
``_axis.SETTINGS_SCHEMA`` so ``Settings/Axis`` edits the same store
``FerroCAD.Axis`` reads. The ``AxisOverlay_Settings`` command is an alias of the
``Settings/Axis`` node whenever an axis provider is registered; the modal dialog
stays as the headless/upstream fallback.

On upstream FreeCAD, where ``FerroCAD.Axis`` does not exist, the command reports
that this build needs FerroCAD's semantic axis API instead of crashing.
"""

from __future__ import annotations

import os

import FreeCADGui

try:
    import FerroCAD
    import FerroCADGui
    from FerroCAD import _axis
except ImportError:  # upstream FreeCAD has no FerroCAD namespace
    FerroCAD = None
    FerroCADGui = None
    _axis = None

ADDON_DIR = os.path.dirname(os.path.abspath(__file__))

#: The provider feature this mod provides, and its backend id.
FEATURE = "axis"
BACKEND = "ferrocad"

#: Reported when ``FerroCAD.Axis`` is absent (that is, on upstream FreeCAD).
UPSTREAM_STATUS = (
    "Axis Overlay needs FerroCAD's semantic axis API (FerroCAD.Axis); this build "
    "does not provide it."
)

#: Reported by ``Activated`` when another backend owns the axis feature.
INACTIVE_STATUS = (
    "Axis Overlay (FerroCAD) is not the active axis provider (%s is active). "
    "Use Mods_Settings to switch backends."
)


def axis_api(app=None):
    """``FerroCAD.Axis`` when this build has it, else ``None``.

    A function so a build without ``Axis`` is detected through the same
    ``getattr`` path a test can stub.
    """
    return getattr(FerroCAD if app is None else app, "Axis", None)


def settings_node():
    """``"Settings/Axis"`` when an axis provider is registered, else ``None``."""
    get_providers = getattr(FerroCADGui, "providers", None)
    if get_providers is None:
        return None
    return "Settings/Axis" if get_providers(FEATURE) else None


def is_active():
    """Whether the FerroCAD-native backend owns the axis feature.

    ``True`` when the facade has no provider registry (upstream FreeCAD) or the
    feature has no registered provider: this mod is then the only axis path.
    """
    active_provider = getattr(FerroCADGui, "activeProvider", None)
    if active_provider is None:
        return True
    return active_provider(FEATURE) in (None, BACKEND)


def inactive_status():
    """The one-line reason ``Activated``/``on_accept`` refuse to run."""
    active_provider = getattr(FerroCADGui, "activeProvider", None)
    active = active_provider(FEATURE) if active_provider is not None else None
    return INACTIVE_STATUS % (active or "another backend")


def activate():
    """Become the drawn axis cross: enable it.

    Returns the ``{"view": …}`` request so a switch repaints at once. The
    registry catches errors; the preference write is idempotent.
    """
    axis = axis_api()
    if axis is None:
        return {"status": UPSTREAM_STATUS}
    axis.enabled = True
    return axis.apply()


def deactivate():
    """Leave the drawn axis cross: hide it, keeping every other preference."""
    axis = axis_api()
    if axis is None:
        return {"status": UPSTREAM_STATUS}
    axis.enabled = False
    return axis.apply()


def _state(axis):
    """The dialog seeds, read back through :meth:`FerroCAD.Axis.snapshot`."""
    return axis.snapshot()


def _field(spec, value):
    """One dialog field for a schema entry (the shell's declarative spec)."""
    field = {
        "name": spec["name"],
        "label": spec["label"],
        "kind": "text" if spec["kind"] == "color" else spec["kind"],
        "value": value,
    }
    for key in ("min", "max"):
        if key in spec:
            field[key] = spec[key]
    return field


def apply_settings(values):
    """Write a dialog's values to ``FerroCAD.Axis`` and return its view request.

    The callback owns the write, as the shell's dialog path expects: it returns
    the ``{"view": …}`` dict :meth:`FerroCAD.Axis.apply` builds. A validation
    error raised by the API stays in the modal.
    """
    if not is_active():
        return {"status": inactive_status()}
    axis = axis_api()
    if axis is None:
        return {"status": UPSTREAM_STATUS}
    for spec in axis.schema():
        key = spec["name"]
        if key not in values:
            continue
        value = values[key]
        if spec["kind"] == "bool":
            setattr(axis, key, bool(value))
        elif spec["kind"] in ("float", "quantity"):
            setattr(axis, key, float(value))
        else:
            setattr(axis, key, str(value))
    return axis.apply()


class AxisOverlayCommand:
    """``AxisOverlay_Settings`` — edit the native axis cross's appearance."""

    def GetResources(self):
        return {
            "MenuText": "Axis Cross",
            "ToolTip": "Show or configure the world axis cross",
            "Group": "View",
            "CmdType": "ForView",
            "FerroCAD_Surface": "axis",
            "FerroCAD_Parent": "App_ToggleAxisCross",
        }

    def IsActive(self):
        return True

    def IsChecked(self):
        """The active axis backend reads as the selected mode."""
        return is_active()

    def Activated(self):
        if not is_active():
            return {"status": inactive_status()}
        node = settings_node()
        if node is not None:
            return {"select": node}
        axis = axis_api()
        if axis is None:
            return {"status": UPSTREAM_STATUS}
        state = _state(axis)
        return {
            "title": "Axis Cross Settings",
            "accept": "Apply",
            "fields": [
                _field(spec, state[spec["name"]])
                for spec in _axis.AXIS_SETTINGS_SCHEMA
            ],
            "on_accept": apply_settings,
        }


# The workbench loader may execute this file more than once (a restart, a test
# boot); ``addCommand`` rejects a duplicate id, so re-registration must be safe.
if "AxisOverlay_Settings" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("AxisOverlay_Settings", AxisOverlayCommand())


# Declare the provider from the mod's entry point. On upstream FreeCAD (no
# FerroCAD provider registry, no ``FerroCAD.Axis``) the mod still loads and
# reports; it simply has nothing to register with. Re-callable, so a test or a
# reset can restore the declaration.
def register_provider():
    register = getattr(FerroCADGui, "registerProvider", None)
    if register is None:
        return False
    register(
        FEATURE,
        BACKEND,
        "FerroCAD (native)",
        command="AxisOverlay_Settings",
        priority=10,
        activate=activate,
        deactivate=deactivate,
        settings=_axis.AXIS_SETTINGS_SCHEMA if _axis is not None else None,
    )
    return True


register_provider()
