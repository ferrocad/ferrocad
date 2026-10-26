# -*- coding: utf-8 -*-
"""Axis Cross (Coin) for FerroCAD: the shader backend of the world axis cross.

The axis cross is its own capability (``FerroCAD.Axis`` + ``Settings/Axis``), not
a grid row. This mod is the **Coin/WGSL** backend of that feature: it builds an
``SoSwitch`` holding an unlit, depth-tested ``SoShaderProgram`` and draws
``Resources/axis_overlay.wgsl`` over a four-vertex placeholder quad. The native
``mods/AxisOverlay`` backend (``axis``/``ferrocad``) draws the same style with
FerroCAD geometry; this backend exists so a Coin-style shader path stays possible.

There is no Qt and no live scene graph. The tree is handed to the shell through
``FerroCADGui.addOverlay(root)``, which extracts it immediately; the shell
re-extracts it on refresh. Because ``addChild`` copies, changing a uniform is
followed by ``_rebind()``: the parameter nodes are written back into the stage,
the separator is rebuilt, and the same root switch is removed from and re-added to
the bridge.

The settings rows are the native ``_axis.SETTINGS_SCHEMA`` with ``get``/``set``
hooks that read and write ``FerroCAD.Axis`` directly, so ``Settings/Axis`` and
both backends share one store; ``enabled`` stays the one shared switch.

This backend is a **visibility pass only**. The ``SoDepthBuffer`` node tests
depth without writing, so model geometry occludes the cross. A single depth pass
cannot also draw the occluded "ghost" stems (``ghost_alpha``), and WGSL cannot
draw text (``labels``), so those two knobs are accepted by the ABI but unused
here; the native backend owns both.
"""

from __future__ import annotations

import os

import FreeCAD as App
import FreeCADGui as Gui

try:
    import FerroCAD as _ferrocad
    import FerroCADGui as _ferrocad_gui
    from FerroCAD import _axis
except ImportError:  # upstream FreeCAD has no FerroCAD namespace
    _ferrocad = None
    _ferrocad_gui = None
    _axis = None

try:
    from pivy import coin
except Exception as exc:  # reported by the command, never fatal at import
    coin = None
    _COIN_ERROR = exc
else:
    _COIN_ERROR = None

ADDON_DIR = os.path.dirname(os.path.abspath(__file__))
RESOURCES = os.path.join(ADDON_DIR, "Resources")
WGSL_PATH = os.path.join(RESOURCES, "axis_overlay.wgsl")

COMMAND = "AxisOverlayCoin_Settings"

#: The provider feature this mod provides, and its backend id. The FerroCAD-native
#: mod declares the same feature with the id ``"ferrocad"`` and priority 10, so it
#: stays the default; see ``docs/workbenches/mod-providers.md``.
FEATURE = "axis"
BACKEND = "coin"

#: Reported when the build has no FerroCAD pull bridge or no semantic axis API.
UPSTREAM_STATUS = (
    "Axis Cross (Coin) needs FerroCAD's pull overlay bridge "
    "(FerroCADGui.addOverlay) and the semantic axis API (FerroCAD.Axis); this "
    "build does not provide them. Use mods/AxisOverlay (native) on this build."
)

#: Reported by ``Activated`` when another backend owns the axis feature.
INACTIVE_STATUS = (
    "Axis Cross (Coin) is not the active axis provider (%s is active). "
    "Use Mods_Settings to switch backends."
)


def axis_api():
    """``FerroCAD.Axis`` when this build has it, else ``None``."""
    return getattr(_ferrocad, "Axis", None)


def overlay_api():
    """The pull bridge, or ``None`` on a build without it."""
    if getattr(_ferrocad_gui, "addOverlay", None) is None:
        return None
    if getattr(_ferrocad_gui, "removeOverlay", None) is None:
        return None
    return _ferrocad_gui


def settings_node():
    """``"Settings/Axis"`` when an axis provider is registered, else ``None``."""
    get_providers = getattr(_ferrocad_gui, "providers", None)
    if get_providers is None:
        return None
    return "Settings/Axis" if get_providers(FEATURE) else None


def is_active():
    """Whether the Coin backend owns the axis feature.

    ``True`` when the facade has no provider registry (upstream FreeCAD) or the
    feature has no registered provider: this mod is then the only axis path.
    """
    active_provider = getattr(_ferrocad_gui, "activeProvider", None)
    if active_provider is None:
        return True
    return active_provider(FEATURE) in (None, BACKEND)


def inactive_status():
    """The one-line reason ``Activated``/``on_accept`` refuse to run."""
    active_provider = getattr(_ferrocad_gui, "activeProvider", None)
    active = active_provider(FEATURE) if active_provider is not None else None
    return INACTIVE_STATUS % (active or "another backend")


def _schema_entry(name):
    """The ``_axis`` schema entry called ``name``, or ``None``."""
    if _axis is None:
        return None
    return next(
        (entry for entry in _axis.AXIS_SETTINGS_SCHEMA if entry["name"] == name),
        None,
    )


def setting(name):
    """Read one axis policy value through ``FerroCAD.Axis``."""
    axis = axis_api()
    if axis is None:
        return None
    return getattr(axis, name)


def save_setting(name, value):
    """Write one axis policy value through ``FerroCAD.Axis``.

    The attribute setters validate (a bad colour raises ``ValueError``), so this
    is the same transaction the native backend's dialog uses.
    """
    axis = axis_api()
    if axis is None:
        raise RuntimeError(UPSTREAM_STATUS)
    spec = _schema_entry(name)
    if spec is None:
        raise ValueError("unknown axis setting %r" % (name,))
    kind = spec["kind"]
    if kind == "bool":
        value = bool(value)
    elif kind in ("float", "quantity"):
        value = float(value)
    else:
        value = str(value)
    setattr(axis, name, value)
    return getattr(axis, name)


def settings_schema():
    """The native axis rows, with get/set hooks into ``FerroCAD.Axis``.

    The vocabulary is ``_axis.AXIS_SETTINGS_SCHEMA`` (labels, kinds, bounds and
    defaults), so ``Settings/Axis`` advertises the same rows whichever axis
    backend is active. The hooks make this mod and ``FerroCAD.Axis`` one store.
    """
    if _axis is None or axis_api() is None:
        return None
    schema = []
    for entry in _axis.AXIS_SETTINGS_SCHEMA:
        name = entry["name"]
        spec = dict(entry)
        spec["get"] = lambda name=name: setting(name)
        spec["set"] = lambda value, name=name: save_setting(name, value)
        schema.append(spec)
    return schema


def _field(spec, value):
    """One dialog field in the shell's declarative spec."""
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


def _srgb_to_linear(channel):
    """One sRGB 0..1 channel in linear light.

    The render target is sRGB, so the ``#rrggbb`` tokens have to be decoded before
    they reach the WGSL block (``docs/workbenches/grid-overlay-plan.md`` §3.4).
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
        raise ValueError("axis colour must be a six-digit hex value (#RRGGBB)")
    try:
        channels = [int(digits[i : i + 2], 16) / 255.0 for i in (0, 2, 4)]
    except ValueError as exc:
        raise ValueError("axis colour must be a six-digit hex value (#RRGGBB)") from exc
    return tuple(_srgb_to_linear(channel) for channel in channels)


def _base_nodes():
    """Pick-proof, unlit state shared by the axis overlay."""
    pick = coin.SoPickStyle()
    pick.style = coin.SoPickStyle.UNPICKABLE
    light = coin.SoLightModel()
    light.model = coin.SoLightModel.BASE_COLOR
    return pick, light


def _depth_read_only():
    """``SoDepthBuffer``: test on, write off, compare ``LESS``.

    Set through ``set()`` because on a node ``.write`` is ``SoBase.write()``, not
    the field. A missing node is reported but not fatal: the program pipeline still
    blends and the overlay just stops occluding.
    """
    try:
        node = coin.SoDepthBuffer()
        node.set("test TRUE")
        node.set("write FALSE")
        node.set("function LESS")
        return node
    except Exception as exc:  # noqa: BLE001 - one optional node is not fatal
        App.Console.PrintWarning(
            "Axis Cross (Coin): SoDepthBuffer unavailable: %s\n" % exc
        )
        return None


class ShaderAxis:
    """The Coin tree: a switch holding an unlit, depth-read-only shader quad."""

    _EPS = 1e-6

    def __init__(self):
        axis = axis_api()
        if axis is None:
            raise RuntimeError(UPSTREAM_STATUS)

        with open(WGSL_PATH, "r", encoding="utf-8") as handle:
            self.wgsl = handle.read()

        # A single-module WGSL file: the vertex stage carries it and the fragment
        # stage stays empty (WGSL has no per-stage modules).
        self.vertex = coin.SoVertexShader()
        self.vertex.source = self.wgsl
        self.fragment = coin.SoFragmentShader()

        # X reuses the grid ink slot; Y and Z have their own slots.
        self.grid_color = coin.SoShaderParameter3f()
        self.grid_color.name = "grid_color"
        self.axis_color_y = coin.SoShaderParameter3f()
        self.axis_color_y.name = "axis_color_y"
        self.axis_color_z = coin.SoShaderParameter3f()
        self.axis_color_z.name = "axis_color_z"
        self.axis_length = coin.SoShaderParameter1f()
        self.axis_length.name = "axis_length"
        self.axis_width = coin.SoShaderParameter1f()
        self.axis_width.name = "axis_width"
        self.axis_opacity = coin.SoShaderParameter1f()
        self.axis_opacity.name = "axis_opacity"
        self.axis_ghost_alpha = coin.SoShaderParameter1f()
        self.axis_ghost_alpha.name = "axis_ghost_alpha"
        self.axis_origin = coin.SoShaderParameter1f()
        self.axis_origin.name = "axis_origin"
        self.axis_labels = coin.SoShaderParameter1f()
        self.axis_labels.name = "axis_labels"

        self.parameters = [
            self.grid_color,
            self.axis_color_y,
            self.axis_color_z,
            self.axis_length,
            self.axis_width,
            self.axis_opacity,
            self.axis_ghost_alpha,
            self.axis_origin,
            self.axis_labels,
        ]

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

        self.depth = _depth_read_only()

        self.root = coin.SoSwitch()
        for node in _base_nodes():
            self.root.addChild(node)
        self.root.whichChild = coin.SO_SWITCH_NONE
        self.sep = None
        self.visible = False
        self._registered = False

        self._push_parameters(axis.snapshot())
        self._rebuild_tree()

    # -- parameters -----------------------------------------------------
    def _push_parameters(self, state):
        """Write the axis policy into the shader parameter nodes."""
        self.grid_color.value = color_to_linear(state["color_x"])
        self.axis_color_y.value = color_to_linear(state["color_y"])
        self.axis_color_z.value = color_to_linear(state["color_z"])
        self.axis_length.value = float(state["length"])
        self.axis_width.value = float(state["width"])
        self.axis_opacity.value = float(state["opacity"])
        # Accepted by the ABI; this backend's single depth pass cannot use it.
        self.axis_ghost_alpha.value = float(state["ghost_alpha"])
        self.axis_origin.value = 1.0 if state["origin"] else 0.0
        # Accepted by the ABI; the native gpui labels are a separate layer.
        self.axis_labels.value = 1.0 if state["labels"] else 0.0

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
        # whole overlay whenever the origin is off screen.
        sep.renderCulling = coin.SoSeparator.OFF
        for child in (self.depth, self.program, self.coords, self.face):
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
    def apply_state(self, state):
        """Rebuild from ``state``, then bind (or unbind) the shared root."""
        self._push_parameters(state)
        self.visible = bool(state["enabled"])
        self.root.whichChild = (
            coin.SO_SWITCH_ALL if self.visible else coin.SO_SWITCH_NONE
        )
        self._rebind()

    def set_visible(self, visible):
        """Attach or detach the switch."""
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


_overlay = None


def overlay():
    """The module's single ``ShaderAxis`` (built on first use), or ``None``."""
    global _overlay
    if _overlay is None:
        if coin is None or axis_api() is None:
            return None
        _overlay = ShaderAxis()
    return _overlay


def apply_dialog(values):
    """Write a dialog's axis values, rebuild the overlay, and return ``apply()``.

    A validation error raised by ``FerroCAD.Axis`` stays in the modal. The
    returned ``{"view": {"axis": …}}`` is what triggers the shell's repaint.
    """
    if not is_active():
        return {"status": inactive_status()}
    axis = axis_api()
    if axis is None:
        return {"status": UPSTREAM_STATUS}
    # Validate colours before persisting, so a bad token leaves the store alone.
    for name in ("color_x", "color_y", "color_z"):
        if name in values:
            color_to_linear(values[name])
    for spec in axis.schema():
        name = spec["name"]
        if name in values:
            save_setting(name, values[name])
    if overlay_api() is not None:
        root = overlay()
        if root is not None:
            root.apply_state(axis.snapshot())
    return axis.apply()


class AxisOverlayCoinCommand:
    """``AxisOverlayCoin_Settings`` — configure the Coin/shader axis cross."""

    def GetResources(self):
        return {
            "MenuText": "Axis Cross (Coin)",
            "ToolTip": "Show or configure the shader (Coin) viewport axis cross",
            "Group": "View",
            "CmdType": "ForView",
            "FerroCAD_Surface": "axis",
            "FerroCAD_Parent": "App_ToggleAxisCross",
            "FerroCAD_DevOnly": "true",
        }

    def IsActive(self):
        return True

    def Activated(self):
        if not is_active():
            return {"status": inactive_status()}
        node = settings_node()
        if node is not None:
            return {"select": node}
        if coin is None:
            return {
                "status": "Axis Cross (Coin) needs the Coin facade (pivy.coin): %s"
                % _COIN_ERROR
            }
        if overlay_api() is None:
            return {"status": UPSTREAM_STATUS}
        axis = axis_api()
        if axis is None:
            return {"status": UPSTREAM_STATUS}
        state = axis.snapshot()
        return {
            "title": "Axis Cross (Coin) Settings",
            "accept": "Apply",
            "fields": [
                _field(spec, state[spec["name"]])
                for spec in _axis.AXIS_SETTINGS_SCHEMA
            ],
            "on_accept": apply_dialog,
        }


def _view(overlay):
    """``FerroCAD.Axis.apply()`` with this backend's native-triad claim.

    ``overlay=True`` tells the shell that this backend draws the axis itself and
    must hide the native triad. The claim follows the stored ``enabled`` switch:
    a disabled axis has nothing to hide, so it claims ``False``.
    """
    request = axis_api().apply()
    request["view"]["axis"]["overlay"] = bool(overlay)
    return request


def activate():
    """Become the drawn axis cross without changing whether it is shown.

    ``FerroCAD.Axis.enabled`` is the feature's switch. This hook only rebuilds
    and binds the overlay from the stored policy, then claims the native hide
    when the axis is actually on. ``activate`` also runs on document activation
    and on a provider switch, so writing the switch here would undo a user's
    "off".
    """
    axis = axis_api()
    if axis is None:
        return {"status": UPSTREAM_STATUS}
    if overlay_api() is None:
        return {"status": UPSTREAM_STATUS}
    root = overlay()
    if root is None:
        return {"status": UPSTREAM_STATUS}
    root.apply_state(axis.snapshot())
    return _view(bool(axis.enabled))


def deactivate():
    """Stop being the drawn axis cross, keeping every preference.

    The overlay is detached and the native-hide claim released; ``enabled`` and
    the appearance values stay exactly as the user left them.
    """
    root = _overlay
    if root is not None:
        root.set_visible(False)
    axis = axis_api()
    return _view(False) if axis is not None else None


def refresh():
    """Re-read the shared store, rebuild and re-register the same root.

    The shell's axis toggle writes ``FerroCAD.Axis.enabled`` and asks the registry
    for a refresh, so the overlay follows the one switch without a second store.
    While this backend is active it owns the native-hide claim, but the claim
    follows ``enabled``: a disabled axis claims no overlay.
    """
    axis = axis_api()
    if axis is None:
        return {"status": UPSTREAM_STATUS}
    root = overlay()
    if root is None:
        return {"status": UPSTREAM_STATUS}
    root.apply_state(axis.snapshot())
    return _view(bool(axis.enabled))


# Registration is idempotent across an add-on reload.
try:
    Gui.removeCommand(COMMAND)
except Exception:  # noqa: BLE001 - absent on the first load
    pass
Gui.addCommand(COMMAND, AxisOverlayCoinCommand())


# Declare the provider from the mod's entry point. On upstream FreeCAD (no
# FerroCAD provider registry or no ``FerroCAD.Axis``) the mod still loads and
# reports; it simply has nothing to register with. Re-callable, so a test or a
# reset can restore the declaration.
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
