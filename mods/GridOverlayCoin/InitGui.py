# -*- coding: utf-8 -*-
"""Grid Overlay (Coin) for FerroCAD: the A1 fork of the upstream add-on.

This is the **Coin/shader** form of the grid overlay (``docs/workbenches/grid-overlay-plan.md``
§3.2, G5): it builds the same ``SoSeparator``/``SoShaderProgram``/``SoFaceSet``
tree the upstream add-on builds and draws the ported WGSL shader on a four-vertex
placeholder quad.

The one deliberate approach change is the **pull bridge**. There is no
``getSceneGraph()``: the tree is handed to the shell through
``FerroCADGui.addOverlay(root)``, which extracts it immediately, and the shell
re-extracts it on refresh. Because ``addChild`` copies (value semantics), changing
a uniform is followed by ``_rebind()``: the parameters are written back into the
stage/program value nodes, the separator is rebuilt, and the same root switch is
removed from and re-added to the bridge.

There is no Qt, no ``QSettings``, no ``getMainWindow`` and no live scene graph.
Settings live in ``FreeCAD.ParamGet`` (in-memory today). Ground shadows keep the
upstream parameters and the 80-slot ABI block but draw nothing: ``shadow_count``
is always 0 (plan §3.5).

The mod declares itself as the ``"coin"`` backend of the ``"grid"`` feature
(priority 0). ``mods/GridOverlay`` declares the same feature as ``"ferrocad"``;
``Mods_Settings`` switches them, and the inactive backend neither draws nor runs
its command (``docs/workbenches/mod-providers.md``).

Since S4 the backend advertises the canonical grid entries it owns through
``registerProvider(..., settings=...)``. The rows read and write through
``setting``/``save_settings``, so ``Settings/Grid`` and the drawn overlay share
one store; ``enabled`` remains ``FerroCAD.Grid.enabled``. The
``GridOverlayCoin_Settings`` command is an alias of that node whenever a grid
provider is registered, and its modal dialog stays as the fallback.
"""

from __future__ import annotations

import os

import FreeCAD as App
import FreeCADGui as Gui

try:
    import FerroCAD as _ferrocad
    import FerroCADGui as _ferrocad_gui
    from FerroCAD import _grid
except ImportError:  # upstream FreeCAD has no FerroCAD namespace
    _ferrocad = None
    _ferrocad_gui = None
    _grid = None

try:
    from pivy import coin
except Exception as exc:  # reported by the command, never fatal at import
    coin = None
    _COIN_ERROR = exc
else:
    _COIN_ERROR = None

ADDON_DIR = os.path.dirname(os.path.abspath(__file__))
RESOURCES = os.path.join(ADDON_DIR, "Resources")
WGSL_PATH = os.path.join(RESOURCES, "grid_overlay.wgsl")

#: The same six knobs and defaults as the upstream add-on (`InitGui.py:134-141`).
DEFAULTS = {
    "enabled": True,
    "opacity": 0.65,
    "color": "#808080",
    "base_spacing": 1.0,
    "shadow_enabled": True,
    "shadow_opacity": 0.30,
}

#: One store, not two: the add-on's ``QSettings`` group becomes a parameter group.
SETTINGS = App.ParamGet("User parameter:BaseApp/Preferences/Mod/GridOverlayCoin")

COMMAND = "GridOverlayCoin_Settings"

#: The provider feature this mod provides, and its backend id. The FerroCAD-native mod
#: declares the same feature with the id ``"ferrocad"``; see
#: ``docs/workbenches/mod-providers.md``.
FEATURE = "grid"
BACKEND = "coin"

#: The canonical schema entries this backend owns, mapped to the key each one
#: uses in this mod's store (``enabled`` is the shared ``FerroCAD.Grid`` switch).
#: ``base_spacing`` is the Coin shader's own scale knobs, not a grid knob, so it
#: is not advertised: no invented entry appears in ``Settings/Grid``.
COIN_SCHEMA_KEYS = {
    "enabled": "enabled",
    "color": "color",
    "opacity": "opacity",
    "shadows": "shadow_enabled",
    "shadow_opacity": "shadow_opacity",
}

UPSTREAM_STATUS = (
    "Grid Overlay (Coin) needs FerroCAD's pull overlay bridge "
    "(FerroCADGui.addOverlay); this build does not provide it. Use the upstream "
    "add-on on upstream FreeCAD."
)


def grid_api():
    """``FerroCAD.Grid`` when this build has the semantic grid API, else ``None``.

    ``FerroCAD.Grid.enabled`` is the one grid switch on FerroCAD, so this mod
    reads it for its master switch exactly as the native backend does. Upstream
    FreeCAD has no such API; there the private ``enabled`` knob stays the master
    switch. This is the same ``getattr`` discriminator as ``mods/GridOverlay``
    (``docs/workbenches/grid-overlay-plan.md`` §3.3).
    """
    return getattr(_ferrocad, "Grid", None)


def setting(key):
    """Read one knob, clamped exactly as the upstream ``setting()`` does.

    ``enabled`` is the shared switch when the semantic API exists, not a knob of
    this mod: the shell's grid toggle owns ``FerroCAD.Grid.enabled`` and the
    overlay has to follow it. On upstream FreeCAD the private knob is the
    fallback.
    """
    if key == "enabled":
        grid = grid_api()
        if grid is not None:
            return bool(grid.enabled)
    if key in ("enabled", "shadow_enabled"):
        return bool(SETTINGS.GetBool(key, DEFAULTS[key]))
    if key == "color":
        return str(SETTINGS.GetString(key, DEFAULTS[key]))
    value = float(SETTINGS.GetFloat(key, DEFAULTS[key]))
    if key == "base_spacing":
        return max(0.01, min(100.0, value))
    return max(0.0, min(1.0, value))


def save_settings(values):
    """Persist the six dialog values; unknown keys are ignored.

    ``enabled`` is written to the shared switch when this build has it, so the
    settings dialog, the toolbar toggle and the drawn grid move together; on
    upstream FreeCAD it stays this mod's own knob.
    """
    if "enabled" in values:
        grid = grid_api()
        if grid is not None:
            grid.enabled = bool(values["enabled"])
        else:
            SETTINGS.SetBool("enabled", bool(values["enabled"]))
    if "color" in values:
        SETTINGS.SetString("color", str(values["color"]))
    if "opacity" in values:
        SETTINGS.SetFloat("opacity", float(values["opacity"]))
    if "base_spacing" in values:
        SETTINGS.SetFloat("base_spacing", float(values["base_spacing"]))
    if "shadow_enabled" in values:
        SETTINGS.SetBool("shadow_enabled", bool(values["shadow_enabled"]))
    if "shadow_opacity" in values:
        SETTINGS.SetFloat("shadow_opacity", float(values["shadow_opacity"]))


def _grid_view(enabled):
    """The view request for one overlay state, carrying the native ``grid`` flag.

    The overlay replaces the native ground while it is bound, so the request
    says both: ``grid`` keeps a checkable command's mark in step with the one
    switch, and ``grid_overlay`` hides the native layer. Without the semantic
    API (upstream) there is nothing to extend but the overlay flag.
    """
    grid = grid_api()
    if grid is None:
        return {"view": {"grid_overlay": bool(enabled)}}
    request = grid.apply()
    request["view"]["grid_overlay"] = bool(enabled)
    return request


def overlay_api():
    """The pull bridge, or ``None`` on a build without it (plan §3.3)."""
    if getattr(_ferrocad_gui, "addOverlay", None) is None:
        return None
    if getattr(_ferrocad_gui, "removeOverlay", None) is None:
        return None
    return _ferrocad_gui


def settings_schema():
    """The canonical entries this backend owns, with store-backed hooks.

    The rows come from ``_grid.SETTINGS_SCHEMA`` (labels, kinds and bounds), so
    the two grid backends advertise the same vocabulary. ``get``/``set`` route
    through this mod's ``setting``/``save_settings``, which is what makes the
    Settings node and the drawn overlay the same store; ``enabled`` is the
    shared ``FerroCAD.Grid`` switch.
    """
    if _grid is None:
        return None
    schema = []
    for entry in _grid.SETTINGS_SCHEMA:
        key = COIN_SCHEMA_KEYS.get(entry["name"])
        if key is None:
            continue
        spec = dict(entry)
        spec["get"] = lambda key=key: setting(key)
        spec["set"] = lambda value, key=key: save_settings({key: value})
        schema.append(spec)
    return schema


def settings_node():
    """``"Settings/Grid"`` when a grid provider is registered, else ``None``."""
    get_providers = getattr(_ferrocad_gui, "providers", None)
    if get_providers is None:
        return None
    return "Settings/Grid" if get_providers(FEATURE) else None


def is_active():
    """Whether the Coin backend owns the grid feature.

    ``True`` when the facade has no provider registry (upstream FreeCAD) or the
    feature has no registered provider: this mod is then the only grid path.
    """
    active_provider = getattr(_ferrocad_gui, "activeProvider", None)
    if active_provider is None:
        return True
    return active_provider(FEATURE) in (None, BACKEND)


def inactive_status():
    """The one-line reason ``Activated``/``on_accept`` refuse to run."""
    active_provider = getattr(_ferrocad_gui, "activeProvider", None)
    active = active_provider(FEATURE) if active_provider is not None else None
    return (
        "Grid Overlay (Coin) is not the active grid provider (%s is active). "
        "Use Mods_Settings to switch backends." % (active or "another backend")
    )


def _field(name, label, kind, value, **extra):
    """One dialog field in the shell's spec (``docs/workbenches/actions-and-tools.md`` §6)."""
    field = {"name": name, "label": label, "kind": kind, "value": value}
    field.update(extra)
    return field


def _srgb_to_linear(channel):
    """One sRGB 0..1 channel in linear light.

    The render target is sRGB, so the add-on's ``#808080`` has to be decoded
    before it reaches the WGSL block (plan §3.4, "Colour space").
    """
    if channel <= 0.04045:
        return channel / 12.92
    return ((channel + 0.055) / 1.055) ** 2.4


def color_to_linear(text):
    """``"#RRGGBB"`` (or ``"RRGGBB"``) as a linear RGB triple.

    A malformed colour raises, so the dialog shows the error rather than silently
    drawing the wrong ink.
    """
    digits = str(text).strip().lstrip("#")
    if len(digits) != 6:
        raise ValueError("grid colour must be a six-digit hex value (#RRGGBB)")
    try:
        channels = [int(digits[i : i + 2], 16) / 255.0 for i in (0, 2, 4)]
    except ValueError as exc:
        raise ValueError("grid colour must be a six-digit hex value (#RRGGBB)") from exc
    return tuple(_srgb_to_linear(channel) for channel in channels)


def _set_mf_floats(field, values):
    """Fill an ``SoMFFloat`` from a Python list (3-arg ``setValues`` then ``setNum``)."""
    try:
        field.setValues(0, len(values), values)
    except Exception:
        for index, value in enumerate(values):
            field.set1Value(index, value)
    field.setNum(len(values))


def _base_nodes():
    """Pick-proof, unlit state shared by the grid and the (deferred) shadow."""
    pick = coin.SoPickStyle()
    pick.style = coin.SoPickStyle.UNPICKABLE
    light = coin.SoLightModel()
    light.model = coin.SoLightModel.BASE_COLOR
    return pick, light


def _depth_no_write():
    """``SoDepthBuffer`` with depth writes off (testing stays on), or ``None``.

    Set through ``set()`` because on a node ``.write`` is ``SoBase.write()``, not
    the field — the upstream add-on's exact workaround.
    """
    try:
        node = coin.SoDepthBuffer()
        node.set("write FALSE")
        return node
    except Exception as exc:  # noqa: BLE001 - one optional node is not fatal
        App.Console.PrintWarning("Grid Overlay (Coin): SoDepthBuffer unavailable: %s\n" % exc)
        return None


def _blend_flag():
    """The transparent-material pass flag, when the Coin surface has it.

    FerroCAD's ``SoMaterial`` has no ``transparency`` field yet, so this is
    optional; the program pipeline blends regardless.
    """
    try:
        flag = coin.SoMaterial()
        flag.transparency.setValue(0.5)
        return flag
    except Exception:  # noqa: BLE001 - a rendering hint, not part of the contract
        return None


class ShaderGrid:
    """The upstream tree: a switch holding an unlit, depth-read-only shader quad."""

    _EPS = 1e-6

    SHADOW_MAX = 64  # must match SHADOW_MAX in grid_overlay.wgsl
    SHADOW_STRIDE = 5  # must match SHADOW_STRIDE in grid_overlay.wgsl

    def __init__(self, document_name=None):
        self.document_name = document_name
        self.visible = False
        self._registered = False

        with open(WGSL_PATH, "r", encoding="utf-8") as handle:
            self.wgsl = handle.read()

        # A single-module WGSL file: the vertex stage carries it and the fragment
        # stage stays empty (WGSL has no per-stage modules; see G3's convention).
        self.vertex = coin.SoVertexShader()
        self.vertex.source = self.wgsl
        self.fragment = coin.SoFragmentShader()

        self.grid_color = coin.SoShaderParameter3f()
        self.grid_color.name = "grid_color"
        self.base_spacing = coin.SoShaderParameter1f()
        self.base_spacing.name = "base_spacing"
        self.minor_alpha = coin.SoShaderParameter1f()
        self.minor_alpha.name = "minor_alpha"
        self.major_alpha = coin.SoShaderParameter1f()
        self.major_alpha.name = "major_alpha"
        self.shadow_count = coin.SoShaderParameter1f()
        self.shadow_count.name = "shadow_count"
        self.shadow_count.value = 0.0
        self.shadow_opacity = coin.SoShaderParameter1f()
        self.shadow_opacity.name = "shadow_opacity"
        self.shadow_opacity.value = 0.0

        self.parameters = [
            self.grid_color,
            self.base_spacing,
            self.minor_alpha,
            self.major_alpha,
            self.shadow_count,
            self.shadow_opacity,
        ]
        self.shadow_data = None
        try:
            self.shadow_data = coin.SoShaderParameterArray1f()
            self.shadow_data.name = "shadow_data"
            _set_mf_floats(
                self.shadow_data.value,
                [0.0] * (self.SHADOW_MAX * self.SHADOW_STRIDE),
            )
            self.parameters.append(self.shadow_data)
        except Exception as exc:  # noqa: BLE001 - the shadow block is deferred
            self.shadow_data = None
            App.Console.PrintWarning(
                "Grid Overlay (Coin): ground shadows unavailable (%s)\n" % exc
            )

        self.program = coin.SoShaderProgram()
        self.program.shaderObject.set1Value(0, self.vertex)
        self.program.shaderObject.set1Value(1, self.fragment)

        epsilon = self._EPS
        self.coords = coin.SoCoordinate3()
        self.coords.point.setValues(
            0,
            4,
            [(-epsilon, -epsilon, 0), (epsilon, -epsilon, 0),
             (epsilon, epsilon, 0), (-epsilon, epsilon, 0)],
        )
        self.face = coin.SoFaceSet()
        self.face.numVertices.setValue(4)

        self.depth = _depth_no_write()
        self.flag = _blend_flag()

        self.root = coin.SoSwitch()
        for node in _base_nodes():
            self.root.addChild(node)
        self.root.whichChild = coin.SO_SWITCH_NONE
        self.sep = None

        self._push_parameters()
        self._rebuild_tree()

    # -- parameters -----------------------------------------------------
    def _push_parameters(self):
        """Write the six knobs into the shader parameter nodes."""
        color = color_to_linear(setting("color"))
        strength = setting("opacity")
        self.grid_color.value = color
        self.base_spacing.value = float(setting("base_spacing"))
        self.minor_alpha.value = strength * 0.35
        self.major_alpha.value = strength
        # Shadows are deferred: the block and the loop exist, the count is zero.
        self.shadow_count.value = 0.0
        self.shadow_opacity.value = float(setting("shadow_opacity"))
        if self.shadow_data is not None:
            _set_mf_floats(
                self.shadow_data.value,
                [0.0] * (self.SHADOW_MAX * self.SHADOW_STRIDE),
            )

    def apply_settings(self):
        """Push settings and re-bind. Call on change only (upstream ``:301-308``)."""
        self._push_parameters()
        self._rebind()

    # -- the value-semantics dance --------------------------------------
    def _rebuild_tree(self):
        """Re-run the value-copied bindings, then refresh the switch's separator.

        ``SoMFNode`` and ``SoGroup.addChild`` copy their input, so the parameter
        nodes in ``root`` are a snapshot. Re-running the two ``set*`` calls and
        re-adding a freshly built separator is what makes the snapshot current
        without an arena or a live graph.
        """
        self.fragment.parameter.setValues(0, len(self.parameters), self.parameters)
        self.program.shaderObject.set1Value(0, self.vertex)
        self.program.shaderObject.set1Value(1, self.fragment)

        sep = coin.SoSeparator()
        # The placeholder sits at the origin; without this Coin would cull the
        # whole grid whenever the origin is off screen.
        sep.renderCulling = coin.SoSeparator.OFF
        for child in (self.depth, self.flag, self.program, self.coords, self.face):
            if child is not None:
                sep.addChild(child)

        if self.sep is not None:
            self.root.removeChild(self.sep)
        self.sep = sep
        self.root.addChild(sep)

    def _bind(self):
        _ferrocad_gui.addOverlay(self.root)
        self._registered = True

    def _unbind(self):
        if self._registered:
            _ferrocad_gui.removeOverlay(self.root)
            self._registered = False

    def _rebind(self):
        """Remove and re-add the same root after a parameter change."""
        self._rebuild_tree()
        if not self.visible:
            self._unbind()
            return
        self._unbind()
        self._bind()

    # -- lifecycle ------------------------------------------------------
    def set_visible(self, visible):
        """Attach or detach the switch (the upstream ``SO_SWITCH_ALL``/``NONE``)."""
        visible = bool(visible)
        self.visible = visible
        self.root.whichChild = (
            coin.SO_SWITCH_ALL if visible else coin.SO_SWITCH_NONE
        )
        if visible:
            if not self._registered:
                self._bind()
        else:
            self._unbind()

    def remove(self):
        """Detach from the bridge, if attached."""
        self._unbind()


class _DocumentObserver:
    """Per-document intent: one grid for the active document."""

    def __init__(self, manager):
        self._manager = manager

    def slotCreatedDocument(self, doc):
        self._manager.ensure_grid()

    def slotActivateDocument(self, doc):
        self._manager.ensure_grid()

    def slotDeletedDocument(self, doc):
        name = getattr(doc, "Name", None)
        if name is not None:
            self._manager.remove_grid(name)


class GridManager:
    """Keeps one ``ShaderGrid`` per open document; only the active one is bound."""

    def __init__(self):
        self.items = {}
        self._observer = _DocumentObserver(self)
        self._observing = False
        try:
            App.addDocumentObserver(self._observer)
            self._observing = True
        except Exception:  # noqa: BLE001 - the facade may not support observers
            pass

    def ensure_grid(self):
        """Create the active document's grid when enabled, then bind it.

        When another backend owns the feature (or the registry is not loaded),
        this is a no-op: only one grid backend draws at a time.
        """
        if not is_active():
            return
        doc = getattr(App, "ActiveDocument", None)
        if doc is None:
            return
        name = getattr(doc, "Name", None)
        if name is None:
            return
        if setting("enabled") and name not in self.items:
            try:
                self.items[name] = ShaderGrid(name)
            except Exception as exc:  # noqa: BLE001 - a broken tree is reported
                App.Console.PrintWarning(
                    "Grid Overlay (Coin): could not build the grid: %s\n" % exc
                )
                return
        self._bind_active()

    def _bind_active(self):
        """Bind only the active document's grid; detach the rest."""
        doc = getattr(App, "ActiveDocument", None)
        active = getattr(doc, "Name", None)
        enabled = setting("enabled")
        for name, grid in self.items.items():
            grid.set_visible(bool(enabled and name == active))

    def apply_settings(self):
        for grid in self.items.values():
            grid.apply_settings()
        self._bind_active()

    def refresh(self):
        self.ensure_grid()
        self.apply_settings()

    def remove_grid(self, name=None):
        names = [name] if name is not None else list(self.items)
        for key in names:
            grid = self.items.pop(key, None)
            if grid is not None:
                grid.remove()

    def shutdown(self):
        if self._observing:
            try:
                App.removeDocumentObserver(self._observer)
            except Exception:  # noqa: BLE001 - teardown is best-effort
                pass
            self._observing = False
        self.remove_grid()


def apply_dialog(values):
    """Persist the dialog's values, re-bind, and ask the shell to redraw.

    The returned ``{"view": {...}}`` is what triggers the shell to pull the
    re-registered overlay. ``grid`` carries the shared switch and
    ``grid_overlay`` hides the native ground while the overlay is bound. A stale
    dialog from a backend that was switched away leaves the store alone and
    reports why.
    """
    if not is_active():
        return {"status": inactive_status()}
    # Validate before persisting, so a bad colour leaves the store alone.
    if "color" in values:
        color_to_linear(values["color"])
    save_settings(values)
    manager.refresh()
    return _grid_view(setting("enabled"))


class GridOverlayCoinCommand:
    """``GridOverlayCoin_Settings`` — configure the Coin/shader grid."""

    def GetResources(self):
        return {
            "MenuText": "Grid Overlay (Coin)",
            "ToolTip": "Show or configure the shader (Coin) viewport grid",
            "Pixmap": os.path.join(RESOURCES, "grid.svg"),
            "Group": "View",
            "CmdType": "ForView",
            "FerroCAD_Surface": "grid",
            "FerroCAD_Parent": "App_ToggleGrid",
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
        if coin is None:
            return {
                "status": "Grid Overlay (Coin) needs the Coin facade (pivy.coin): %s"
                % _COIN_ERROR
            }
        if overlay_api() is None:
            return {"status": UPSTREAM_STATUS}
        manager.ensure_grid()
        return {
            "title": "Grid Overlay (Coin) Settings",
            "accept": "Apply",
            "fields": [
                _field("enabled", "Show custom grid", "bool", setting("enabled")),
                _field("color", "Grid colour", "text", setting("color")),
                _field(
                    "opacity",
                    "Opacity",
                    "float",
                    setting("opacity"),
                    min=0.0,
                    max=1.0,
                ),
                _field(
                    "base_spacing",
                    "Base spacing scale",
                    "quantity",
                    setting("base_spacing"),
                    min=0.01,
                    max=100.0,
                ),
                _field(
                    "shadow_enabled",
                    "Show ground shadow",
                    "bool",
                    setting("shadow_enabled"),
                ),
                _field(
                    "shadow_opacity",
                    "Ground shadow opacity",
                    "float",
                    setting("shadow_opacity"),
                    min=0.0,
                    max=1.0,
                ),
            ],
            "on_accept": apply_dialog,
        }


_previous = getattr(Gui, "_GridOverlayCoinManager", None)
if _previous is not None:
    try:
        _previous.shutdown()
    except Exception:  # noqa: BLE001 - a stale manager must not block reload
        pass
manager = GridManager()
Gui._GridOverlayCoinManager = manager


def activate():
    """Become the drawn grid: turn the shared switch on and bind the overlay.

    The switch is the one ``FerroCAD.Grid`` store, so taking the grid over turns
    it on; the returned request rebinds the overlay and hides the native ground
    in the same repaint. The private knob is only the upstream fallback.
    """
    grid = grid_api()
    if grid is None:
        manager.ensure_grid()
        return None
    grid.enabled = True
    manager.ensure_grid()
    return _grid_view(True)


def deactivate():
    """Detach every overlay this mod registered and restore the hidden default.

    Only visibility moves: the appearance knobs stay, so switching back is
    lossless (the ``deactivate`` contract in ``docs/workbenches/mod-providers.md``).
    """
    manager.remove_grid()
    grid = grid_api()
    if grid is None:
        return None
    grid.enabled = False
    return _grid_view(False)


def refresh():
    """Re-read the shared switch and (un)bind the overlay (the provider hook).

    The shell's grid toggle flips ``FerroCAD.Grid.enabled`` and then asks the
    registry for a refresh, so the overlay follows the one switch without a
    second store. The ``grid_overlay`` claim in the result is what the toggle
    folds into its view request.
    """
    manager.refresh()
    return {"view": {"grid_overlay": bool(setting("enabled"))}}


# Registration is idempotent across an add-on reload (the facade rejects a
# duplicate id rather than overwriting it).
try:
    Gui.removeCommand(COMMAND)
except Exception:  # noqa: BLE001 - absent on the first load
    pass
Gui.addCommand(COMMAND, GridOverlayCoinCommand())

# Attach on startup when a document is already open; otherwise wait for one.
if coin is not None and setting("enabled") and getattr(App, "ActiveDocument", None):
    manager.ensure_grid()

# Declare the provider from the mod's entry point. On upstream FreeCAD (no
# FerroCAD provider registry) the mod still loads and reports; it simply has
# nothing to register with. Re-callable, so a test or a reset can restore the
# declaration.
def register_provider():
    register = getattr(_ferrocad_gui, "registerProvider", None)
    if register is None:
        return False
    register(
        FEATURE,
        BACKEND,
        "Coin (WGSL)",
        command=COMMAND,
        priority=0,
        activate=activate,
        deactivate=deactivate,
        refresh=refresh,
        settings=settings_schema(),
    )
    return True


register_provider()
