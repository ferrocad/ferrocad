# -*- coding: utf-8 -*-
"""Gizmo Overlay (shader) for FerroCAD: the shader-skin backend of the gizmos.

The gizmo is its own capability (``FerroCAD.Gizmo`` + ``Settings/Gizmo``), not a
scene-graph addon. This mod is the **shader** backend of that feature: it claims
the backend id ``"ferrocad-shader"`` so the provider registry (header popovers,
Dev Tools -> Providers) can switch the drawn handle geometry to the skin this
mod ships, ``Resources/gizmo_skin.wgsl``. The native ``mods/GizmoOverlay``
backend (``gizmo``/``ferrocad``, priority 10) stays the default and the
hit-test/drag owner.

The skin is this mod's own material program, not the app's. It is handed to the
registry through ``registerProvider(..., skin=...)``, and the host compiles
whatever source the **active** backend supplies (``FerroCADGui.providerSkin``
asks the active backend) into a geometry program. The file must therefore
satisfy the scene's ABI: it is a geometry program (the packet carries the
mesh), it declares the scene ``Globals`` at ``@group(0)`` and ``Frame`` at
``@group(2)``, and it consumes the scene vertex attributes ``position``,
``normal``, ``uv`` and ``color``. The source is read once at import; if the
file is missing or unreadable, the mod says so once on stderr and still
registers without a skin, so the host draws its native material.

There is no Qt, no ``pivy`` and no Coin tree. A Python overlay cannot track the
gizmo's per-selection frame, so the host draws the native gizmo geometry with
this material. Switching skins is visual: the native
``TransformGizmo``/manipulators keep hover, picking, drags and commits.

The settings rows are the canonical ``_gizmo.GIZMO_SETTINGS_SCHEMA``, so
``Settings/Gizmo`` and both backends are one store (``FerroCAD.Gizmo``).
``GizmoOverlayShader_Settings`` selects ``Settings/Gizmo`` when a gizmo provider
is registered; its declarative fallback dialog writes ``FerroCAD.Gizmo`` and
returns ``FerroCAD.Gizmo.apply()``.

On upstream FreeCAD, where ``FerroCAD.Gizmo`` does not exist, the command
reports that this build needs FerroCAD's semantic gizmo API instead of crashing.
"""

from __future__ import annotations

import os
import sys

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
RESOURCES = os.path.join(ADDON_DIR, "Resources")
WGSL_PATH = os.path.join(RESOURCES, "gizmo_skin.wgsl")

#: The provider feature this mod provides, and its backend id.
FEATURE = "gizmo"
BACKEND = "ferrocad-shader"

#: The skin text, cached by :func:`skin_source` on its first call (at import).
_SKIN = None
_SKIN_READ = False

#: Reported when ``FerroCAD.Gizmo`` is absent (that is, on upstream FreeCAD).
UPSTREAM_STATUS = (
    "Gizmo Overlay (shader) needs FerroCAD's semantic gizmo API (FerroCAD.Gizmo); "
    "this build does not provide it."
)


def _read_skin():
    """Read the shipped WGSL text; an ``OSError`` means "no skin"."""
    with open(WGSL_PATH, "r", encoding="utf-8") as handle:
        return handle.read()


def skin_source():
    """The WGSL material program this mod ships, or ``None``.

    Read once, at import, and cached in a module global: registration passes
    the same source, so a re-registration never touches the disk again. A
    missing or unreadable ``Resources/gizmo_skin.wgsl`` is reported once on
    stderr and answers ``None`` instead of breaking the mod; the provider still
    registers and the host falls back to its native material.
    """
    global _SKIN, _SKIN_READ
    if not _SKIN_READ:
        _SKIN_READ = True
        try:
            _SKIN = _read_skin()
        except OSError as exc:
            _SKIN = None
            print(
                f"GizmoOverlayShader: cannot read {WGSL_PATH}: {exc}",
                file=sys.stderr,
            )
    return _SKIN


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
    """Whether the shader backend owns the gizmo feature.

    ``True`` when the facade has no provider registry (upstream FreeCAD) or the
    feature has no registered provider: this mod is then the only gizmo path.
    """
    active_provider = getattr(FerroCADGui, "activeProvider", None)
    if active_provider is None:
        return True
    return active_provider(FEATURE) in (None, BACKEND)


def _view():
    """``FerroCAD.Gizmo.apply()`` with this backend's explicit claim.

    The shader provider answers for itself, so the request always names
    ``"ferrocad-shader"`` even while another backend happens to be the active
    one.
    """
    request = gizmo_api().apply()
    request["view"]["gizmo"]["backend"] = BACKEND
    return request


def activate():
    """Become the drawn gizmo without changing whether it is shown.

    ``FerroCAD.Gizmo.enabled`` is the feature's switch. This hook only claims the
    shader backend, so a document activation or a backend switch cannot undo a
    user's "off".
    """
    if gizmo_api() is None:
        return {"status": UPSTREAM_STATUS}
    return _view()


def deactivate():
    """Stop being the drawn gizmo, keeping every preference.

    The skin is data, so leaving it only repeats the backend claim. ``enabled``
    and the appearance values stay exactly as the user left them.
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


class GizmoOverlayShaderCommand:
    """``GizmoOverlayShader_Settings`` — edit the gizmo skin and appearance."""

    def GetResources(self):
        return {
            "MenuText": "Gizmo (shader)",
            "ToolTip": "Configure the manipulator gizmos (compiled shader skin)",
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
if "GizmoOverlayShader_Settings" not in FreeCADGui.listCommands():
    FreeCADGui.addCommand(
        "GizmoOverlayShader_Settings", GizmoOverlayShaderCommand()
    )


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
        "FerroCAD (shader)",
        command="GizmoOverlayShader_Settings",
        priority=0,
        activate=activate,
        deactivate=deactivate,
        refresh=refresh,
        settings=_gizmo.GIZMO_SETTINGS_SCHEMA if _gizmo is not None else None,
        skin=skin_source(),
    )
    return True


# Read the skin now so the import, not a later re-registration, reports a
# missing file; ``register_provider`` then only passes the cached source.
skin_source()

register_provider()
