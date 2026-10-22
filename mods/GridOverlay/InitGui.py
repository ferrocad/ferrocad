# -*- coding: utf-8 -*-
"""Grid Overlay for FerroCAD: a dialog for the native reference grid.

This is the A2 shipping form (``docs/workbenches/grid-overlay-plan.md`` §3.2), not a port of
the upstream Coin/GLSL overlay. The command id and menu text are unchanged and
the six knobs survive, but the values go to the semantic ``FerroCAD.Grid`` API
and the shell draws them with the native grid. There is no Qt, no ``pivy``, no
scene graph and no per-document overlay tree.

On upstream FreeCAD, where ``FerroCAD.Grid`` does not exist, the command reports
that this build needs FerroCAD's semantic grid API rather than falling back to
the legacy Coin path; the upstream addon remains the addon for upstream.

The mod declares itself as the ``"ferrocad"`` backend of the ``"grid"`` feature
(priority 10, so the native grid is the default), advertising the canonical
``_grid.SETTINGS_SCHEMA`` so ``Settings/Grid`` edits the same store
``FerroCAD.Grid`` reads. ``mods/GridOverlayCoin`` declares the same feature as
``"coin"``; only one is active at a time, and ``Mods_Settings`` switches them
(``docs/workbenches/mod-providers.md``). The ``GridOverlay_Settings`` command is
an alias of the ``Settings/Grid`` node whenever a grid provider is registered;
the modal dialog stays as the headless/upstream fallback.
"""

from __future__ import annotations

import os

import FreeCADGui

try:
    import FerroCAD
    import FerroCADGui
    from FerroCAD import _grid
except ImportError:  # upstream FreeCAD has no FerroCAD namespace
    FerroCAD = None
    FerroCADGui = None
    _grid = None

ADDON_DIR = os.path.dirname(os.path.abspath(__file__))

#: The provider feature this mod provides, and its backend id. The Coin mod
#: declares the same feature with the id ``"coin"``; see ``docs/workbenches/mod-providers.md``.
FEATURE = "grid"
BACKEND = "ferrocad"

#: Reported when ``FerroCAD.Grid`` is absent (that is, on upstream FreeCAD).
UPSTREAM_STATUS = (
    "Grid Overlay needs FerroCAD's semantic grid API (FerroCAD.Grid); this build "
    "does not provide it. Use the upstream addon on upstream FreeCAD."
)

#: Reported by ``Activated`` when the Coin backend owns the grid.
INACTIVE_STATUS = (
    "Grid Overlay (FerroCAD) is not the active grid provider (%s is active). "
    "Use Mods_Settings to switch backends."
)


def grid_api(app=None):
    """``FerroCAD.Grid`` when this build has it, else ``None``.

    The discriminator from ``docs/workbenches/grid-overlay-plan.md`` §3.3. It is a function
    so a build without ``Grid`` is detected through the same ``getattr`` path
    the plan names, and so a test can pass a module that lacks the attribute.
    """
    return getattr(FerroCAD if app is None else app, "Grid", None)


def settings_node():
    """``"Settings/Grid"`` when a grid provider is registered, else ``None``.

    The Settings node is the single grid-settings surface (S4); the mod's own
    dialog is only the fallback for a build with no provider registry.
    """
    get_providers = getattr(FerroCADGui, "providers", None)
    if get_providers is None:
        return None
    return "Settings/Grid" if get_providers(FEATURE) else None


def is_active():
    """Whether the FerroCAD-native backend owns the grid feature.

    ``True`` when the facade has no provider registry (upstream FreeCAD) or the
    feature has no registered provider: this mod is then the only grid path.
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
    """Become the drawn grid: enable the native grid.

    Returns the ``{"view": …}`` request so a switch repaints at once. The
    registry catches errors; the preference write is idempotent.
    """
    grid = grid_api()
    if grid is None:
        return {"status": UPSTREAM_STATUS}
    grid.enabled = True
    return grid.apply()


def deactivate():
    """Leave the drawn grid: restore the native grid's default (hidden).

    Only visibility is restored; the user's colour and spacing preferences are
    left alone, so switching back is lossless.
    """
    grid = grid_api()
    if grid is None:
        return {"status": UPSTREAM_STATUS}
    grid.enabled = False
    return grid.apply()


def _state(grid):
    """The six dialog seeds, read back through :meth:`FerroCAD.Grid.snapshot`.

    ``snapshot()`` carries the four applied knobs; the shadow preferences are
    persisted but have no view field yet, so they are read directly.
    """
    snapshot = grid.snapshot()
    return {
        "enabled": bool(snapshot["grid"]),
        "color": str(snapshot["grid_color"]),
        "opacity": float(snapshot["grid_alpha"]),
        "spacing": float(snapshot["grid_minor"]),
        "shadows": bool(grid.shadows),
        "shadow_opacity": float(grid.shadow_opacity),
    }


def _field(name, label, kind, value, **extra):
    """One dialog field in the shell's spec (``docs/workbenches/actions-and-tools.md`` §6)."""
    field = {"name": name, "label": label, "kind": kind, "value": value}
    field.update(extra)
    return field


def apply_settings(values):
    """Write a dialog's values to ``FerroCAD.Grid`` and return its view request.

    The callback owns the write, as the shell's dialog path expects: it returns
    the ``{"view": …}`` dict :meth:`FerroCAD.Grid.apply` builds, which the shell
    applies and mirrors. A validation error raised by the API stays in the
    modal.
    """
    if not is_active():
        return {"status": inactive_status()}
    grid = grid_api()
    if grid is None:
        return {"status": UPSTREAM_STATUS}
    if "enabled" in values:
        grid.enabled = bool(values["enabled"])
    if "color" in values:
        grid.color = str(values["color"])
    if "opacity" in values:
        grid.opacity = float(values["opacity"])
    if "spacing" in values:
        grid.spacing = float(values["spacing"])
    if "shadows" in values:
        grid.shadows = bool(values["shadows"])
    if "shadow_opacity" in values:
        grid.shadow_opacity = float(values["shadow_opacity"])
    return grid.apply()


class GridOverlayCommand:
    """``GridOverlay_Settings`` — edit the native grid's appearance."""

    def GetResources(self):
        return {
            "MenuText": "Grid Overlay",
            "ToolTip": "Show or configure the viewport grid",
            "Pixmap": os.path.join(ADDON_DIR, "Resources", "grid.svg"),
            "Group": "View",
            "CmdType": "ForView",
            "FerroCAD_Surface": "grid",
            "FerroCAD_Parent": "App_ToggleGrid",
            "FerroCAD_DevOnly": "true",
        }

    def IsActive(self):
        return True

    def IsChecked(self):
        """The active grid backend reads as the selected mode in the Grid menu."""
        return is_active()

    def Activated(self):
        if not is_active():
            return {"status": inactive_status()}
        node = settings_node()
        if node is not None:
            return {"select": node}
        grid = grid_api()
        if grid is None:
            return {"status": UPSTREAM_STATUS}
        state = _state(grid)
        return {
            "title": "Grid Overlay Settings",
            "accept": "Apply",
            "fields": [
                _field("enabled", "Show grid", "bool", state["enabled"]),
                _field("color", "Grid colour", "text", state["color"]),
                _field(
                    "opacity",
                    "Opacity",
                    "float",
                    state["opacity"],
                    min=0.0,
                    max=1.0,
                ),
                _field(
                    "spacing",
                    "Base spacing (mm)",
                    "quantity",
                    state["spacing"],
                    min=0.001,
                ),
                _field("shadows", "Show ground shadow", "bool", state["shadows"]),
                _field(
                    "shadow_opacity",
                    "Ground shadow opacity",
                    "float",
                    state["shadow_opacity"],
                    min=0.0,
                    max=1.0,
                ),
            ],
            "on_accept": apply_settings,
        }


# The workbench loader may execute this file more than once (a restart, a test
# boot); ``addCommand`` rejects a duplicate id, so re-registration must be safe.
if "GridOverlay_Settings" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand("GridOverlay_Settings", GridOverlayCommand())

# Declare the provider from the mod's entry point. On upstream FreeCAD (no
# FerroCAD provider registry, no ``FerroCAD.Grid``) the mod still loads and
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
        command="GridOverlay_Settings",
        priority=10,
        activate=activate,
        deactivate=deactivate,
        settings=_grid.SETTINGS_SCHEMA if _grid is not None else None,
    )
    return True


register_provider()
