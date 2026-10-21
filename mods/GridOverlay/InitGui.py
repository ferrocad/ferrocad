# -*- coding: utf-8 -*-
"""Grid Overlay for FerroCAD: a dialog for the native reference grid.

This is the A2 shipping form (``docs/grid-overlay-plan.md`` §3.2), not a port of
the upstream Coin/GLSL overlay. The command id and menu text are unchanged and
the six knobs survive, but the values go to the semantic ``FreeCAD.Grid`` API
and the shell draws them with the native grid. There is no Qt, no ``pivy``, no
scene graph and no per-document overlay tree.

On upstream FreeCAD, where ``FreeCAD.Grid`` does not exist, the command reports
that this build needs FerroCAD's semantic grid API rather than falling back to
the legacy Coin path; the upstream addon remains the addon for upstream.
"""

from __future__ import annotations

import os

import FreeCAD
import FreeCADGui

ADDON_DIR = os.path.dirname(os.path.abspath(__file__))

#: Reported when ``FreeCAD.Grid`` is absent (that is, on upstream FreeCAD).
UPSTREAM_STATUS = (
    "Grid Overlay needs FerroCAD's semantic grid API (FreeCAD.Grid); this build "
    "does not provide it. Use the upstream addon on upstream FreeCAD."
)


def grid_api(app=None):
    """``FreeCAD.Grid`` when this build has it, else ``None``.

    The discriminator from ``docs/grid-overlay-plan.md`` §3.3. It is a function
    so a build without ``Grid`` is detected through the same ``getattr`` path
    the plan names, and so a test can pass a module that lacks the attribute.
    """
    return getattr(FreeCAD if app is None else app, "Grid", None)


def _state(grid):
    """The six dialog seeds, read back through :meth:`FreeCAD.Grid.snapshot`.

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
    """One dialog field in the shell's spec (``docs/actions-and-tools.md`` §6)."""
    field = {"name": name, "label": label, "kind": kind, "value": value}
    field.update(extra)
    return field


def apply_settings(values):
    """Write a dialog's values to ``FreeCAD.Grid`` and return its view request.

    The callback owns the write, as the shell's dialog path expects: it returns
    the ``{"view": …}`` dict :meth:`FreeCAD.Grid.apply` builds, which the shell
    applies and mirrors. A validation error raised by the API stays in the
    modal.
    """
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
        }

    def IsActive(self):
        return True

    def Activated(self):
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


FreeCADGui.addCommand("GridOverlay_Settings", GridOverlayCommand())
