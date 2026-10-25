# -*- coding: utf-8 -*-
"""Gizmo Overlay for FerroCAD: settings for the manipulator gizmos.

The command (``GizmoOverlay_Settings``) is the gizmo's one settings surface.
Like ``mods/GridOverlay`` and ``mods/AxisOverlay`` it is a thin client of the
semantic API rather than a scene-graph addon: there is no Qt, no ``pivy`` and no
per-document overlay tree. The values go to ``FerroCAD.Gizmo`` and the shell
draws the gizmo with its native manipulator overlay.

The mod declares itself as the ``"ferrocad"`` backend of the ``"gizmo"``
feature (priority 10, so the native gizmo is the default), advertising the
canonical ``_gizmo.GIZMO_SETTINGS_SCHEMA`` so ``Settings/Gizmo`` edits the same
store ``FerroCAD.Gizmo`` reads. The ``GizmoOverlay_Settings`` command is an
alias of the ``Settings/Gizmo`` node whenever a gizmo provider is registered;
the modal dialog stays as the headless/upstream fallback.

On upstream FreeCAD, where ``FerroCAD.Gizmo`` does not exist, the command
reports that this build needs FerroCAD's semantic gizmo API instead of crashing.
"""

from __future__ import annotations

import os

import FreeCADGui

try:
    import FerroCAD
    import FerroCADGui
    from FerroCAD import _gizmo
except ImportError:  # upstream FreeCAD has no FerroCAD namespace
    FerroCAD = None
    FerroCADGui = None
    _gizmo = None

ADDON_DIR = os.path.dirname(os.path.abspath(__file__))

#: The provider feature this mod provides, and its backend id.
FEATURE = "gizmo"
BACKEND = "ferrocad"

#: Reported when ``FerroCAD.Gizmo`` is absent (that is, on upstream FreeCAD).
UPSTREAM_STATUS = (
    "Gizmo Overlay needs FerroCAD's semantic gizmo API (FerroCAD.Gizmo); this "
    "build does not provide it."
)


def gizmo_api(app=None):
    """``FerroCAD.Gizmo`` when this build has it, else ``None``.

    A function so a build without ``Gizmo`` is detected through the same
    ``getattr`` path a test can stub.
    """
    return getattr(FerroCAD if app is None else app, "Gizmo", None)


def settings_node():
    """``"Settings/Gizmo"`` when a gizmo provider is registered, else ``None``."""
    get_providers = getattr(FerroCADGui, "providers", None)
    if get_providers is None:
        return None
    return "Settings/Gizmo" if get_providers(FEATURE) else None


def is_active():
    """Whether the FerroCAD-native backend owns the gizmo feature.

    ``True`` when the facade has no provider registry (upstream FreeCAD) or the
    feature has no registered provider: this mod is then the only gizmo path.
    """
    active_provider = getattr(FerroCADGui, "activeProvider", None)
    if active_provider is None:
        return True
    return active_provider(FEATURE) in (None, BACKEND)


def _view():
    """``FerroCAD.Gizmo.apply()`` with this backend's explicit claim.

    The native provider answers for itself, so the request always names
    ``"ferrocad"`` even while another backend happens to be the active one.
    """
    request = gizmo_api().apply()
    request["view"]["gizmo"]["backend"] = BACKEND
    return request


def activate():
    """Become the drawn gizmo without changing whether it is shown.

    ``FerroCAD.Gizmo.enabled`` is the feature's switch. This hook only claims the
    native implementation, so a document activation or a backend switch cannot
    undo a user's "off".
    """
    if gizmo_api() is None:
        return {"status": UPSTREAM_STATUS}
    return _view()


def deactivate():
    """Stop being the drawn gizmo, keeping every preference.

    The native backend adds nothing to the viewport, so leaving it is a no-op
    apart from repeating the ``"ferrocad"`` backend claim. ``enabled`` and the
    appearance values stay exactly as the user left them.
    """
    if gizmo_api() is None:
        return {"status": UPSTREAM_STATUS}
    return _view()


def refresh():
    """Re-read the shared store after the shell or the pane changed it."""
    if gizmo_api() is None:
        return {"status": UPSTREAM_STATUS}
    return _view()


def _state(gizmo):
    """The dialog seeds, read back through :meth:`FerroCAD.Gizmo.snapshot`."""
    return gizmo.snapshot()


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
    """Write a dialog's values to ``FerroCAD.Gizmo`` and return its view request.

    The callback owns the write, as the shell's dialog path expects: it returns
    the ``{"view": …}`` dict :meth:`FerroCAD.Gizmo.apply` builds. A validation
    error raised by the API stays in the modal.
    """
    gizmo = gizmo_api()
    if gizmo is None:
        return {"status": UPSTREAM_STATUS}
    for spec in gizmo.schema():
        key = spec["name"]
        if key not in values:
            continue
        value = values[key]
        if spec["kind"] == "bool":
            setattr(gizmo, key, bool(value))
        elif spec["kind"] in ("float", "quantity"):
            setattr(gizmo, key, float(value))
        else:
            setattr(gizmo, key, str(value))
    return gizmo.apply()


class GizmoOverlayCommand:
    """``GizmoOverlay_Settings`` — edit the native gizmo's appearance."""

    def GetResources(self):
        return {
            "MenuText": "Gizmo",
            "ToolTip": "Configure the manipulator gizmos",
            "Group": "View",
            "CmdType": "ForView",
            "FerroCAD_Surface": "gizmo",
            "FerroCAD_Parent": "",
            "FerroCAD_DevOnly": "true",
        }

    def IsActive(self):
        return True

    def IsChecked(self):
        """The active gizmo backend reads as the selected mode."""
        return is_active()

    def Activated(self):
        node = settings_node()
        if node is not None:
            return {"select": node}
        gizmo = gizmo_api()
        if gizmo is None:
            return {"status": UPSTREAM_STATUS}
        state = _state(gizmo)
        return {
            "title": "Gizmo Settings",
            "accept": "Apply",
            "fields": [
                _field(spec, state[spec["name"]])
                for spec in _gizmo.GIZMO_SETTINGS_SCHEMA
            ],
            "on_accept": apply_settings,
        }


# The workbench loader may execute this file more than once (a restart, a test
# boot); ``addCommand`` rejects a duplicate id, so re-registration must be safe.
if "GizmoOverlay_Settings" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("GizmoOverlay_Settings", GizmoOverlayCommand())


# Declare the provider from the mod's entry point. On upstream FreeCAD (no
# FerroCAD provider registry, no ``FerroCAD.Gizmo``) the mod still loads and
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
        command="GizmoOverlay_Settings",
        priority=10,
        activate=activate,
        deactivate=deactivate,
        refresh=refresh,
        settings=_gizmo.GIZMO_SETTINGS_SCHEMA if _gizmo is not None else None,
    )
    return True


register_provider()
