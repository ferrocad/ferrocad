"""Tests for the AxisOverlayCoin mod (the Coin/WGSL axis-cross backend).

The mod is the shader form of the axis cross, carried by FerroCAD's **pull**
overlay bridge: activating rebuilds the shader parameters from
``FerroCAD.Axis.snapshot()`` and calls ``FerroCADGui.addOverlay(root)``;
deactivating removes it, and a refresh rebuilds and re-registers the same root.
The WGSL file in the mod is a duplicate of the crate's reference copy and must
stay byte-identical.

Headless (``FreeCAD.GuiUp == 0``): the test installs a small ``pivy.coin`` stub so
the mod can build its tree without the Rust extension, then spies on the facade's
overlay functions. Nothing here needs a GPU.
"""

import importlib.util
import pathlib
import sys
import types
import unittest
from unittest import mock

import FreeCAD
import FreeCADGui
import FerroCAD
import FerroCADGui
from FerroCAD import _axis

MOD_DIR = pathlib.Path(__file__).resolve().parents[1] / "mods" / "AxisOverlayCoin"
MOD_PATH = MOD_DIR / "InitGui.py"
MOD_WGSL = MOD_DIR / "Resources" / "axis_overlay.wgsl"
CRATE_WGSL = (
    pathlib.Path(__file__).resolve().parents[1]
    / "crates"
    / "ferrocad_wgpu"
    / "shaders"
    / "axis_overlay.wgsl"
)
COMMAND = "AxisOverlayCoin_Settings"
PREFERENCES = "User parameter:BaseApp/Preferences/Mods/axis/ferrocad"
GRID_PREFERENCES = "User parameter:BaseApp/Preferences/Mods/grid/ferrocad"
LEGACY_PREFERENCES = "User parameter:BaseApp/Preferences/View"


# ---------------------------------------------------------------------------
# A minimal ``pivy.coin`` stub: fields, groups and the constants a Coin tree uses
# ---------------------------------------------------------------------------


class _FakeField:
    """A Coin field with the arities the mod uses."""

    def __init__(self, *values):
        self.values = list(values)

    def setValue(self, value):
        if self.values:
            self.values[0] = value
        else:
            self.values.append(value)

    def getValue(self):
        return self.values[0] if self.values else None

    def setValues(self, start, *args):
        if len(args) == 1:
            values = list(args[0])
        else:
            num, values = args
            values = list(values)[: int(num)]
        end = start + len(values)
        if len(self.values) < end:
            self.values.extend([0.0] * (end - len(self.values)))
        self.values[start:end] = values

    def set1Value(self, index, value):
        if len(self.values) <= index:
            self.values.extend([None] * (index + 1 - len(self.values)))
        self.values[index] = value

    def setNum(self, num):
        num = int(num)
        if num < len(self.values):
            del self.values[num:]
        else:
            self.values.extend([0.0] * (num - len(self.values)))

    def __len__(self):
        return len(self.values)


class _FakeNode:
    """A structural stand-in for any Coin node the mod constructs."""

    def __init__(self):
        self.children = []
        self.whichChild = -3
        self.renderCulling = 0
        self.source = ""
        self.sourceProgram = ""
        self.name = ""
        self.value = None
        self.style = None
        self.model = None
        self.test = True
        self.write = True
        self.function = 0
        self.point = _FakeField()
        self.numVertices = _FakeField()
        self.parameter = _FakeField()
        self.shaderObject = _FakeField()

    def addChild(self, child):
        self.children.append(child)

    def removeChild(self, child):
        for index, candidate in enumerate(self.children):
            if candidate is child:
                del self.children[index]
                return

    def set(self, spec):
        field, value = spec.split(None, 1)
        text = value.strip()
        if field == "function":
            # The mod uses ``set("function LESS")``; keep the enum ordinal.
            functions = {"NEVER": 0, "ALWAYS": 1, "LESS": 2, "EQUAL": 3}
            setattr(self, field, functions.get(text.upper(), 0))
            return
        setattr(self, field, text.lower() in ("true", "1", "yes", "on"))


def _fake_type(name, **attributes):
    return type(name, (_FakeNode,), attributes)


def _build_fake_coin():
    module = types.ModuleType("pivy.coin")
    module.SO_SWITCH_NONE = -1
    module.SO_SWITCH_INHERIT = -2
    module.SO_SWITCH_ALL = -3
    module.SoSeparator = _fake_type("SoSeparator", OFF=0, AUTO=1, ON=2)
    module.SoSwitch = _fake_type("SoSwitch")
    module.SoDepthBuffer = _fake_type(
        "SoDepthBuffer", NEVER=0, ALWAYS=1, LESS=2, EQUAL=3
    )
    module.SoVertexShader = _fake_type("SoVertexShader")
    module.SoFragmentShader = _fake_type("SoFragmentShader")
    module.SoShaderProgram = _fake_type("SoShaderProgram")
    module.SoCoordinate3 = _fake_type("SoCoordinate3")
    module.SoFaceSet = _fake_type("SoFaceSet")
    module.SoPickStyle = _fake_type("SoPickStyle", UNPICKABLE=0, SHAPE=1)
    module.SoLightModel = _fake_type("SoLightModel", BASE_COLOR=0, PHONG=1)
    module.SoShaderParameter1f = _fake_type("SoShaderParameter1f")
    module.SoShaderParameter3f = _fake_type("SoShaderParameter3f")
    module._registered = []

    def add_overlay(node):
        module._registered.append(node)

    def remove_overlay(node):
        for index, candidate in enumerate(module._registered):
            if candidate is node:
                del module._registered[index]
                return True
        return False

    module.addOverlay = add_overlay
    module.removeOverlay = remove_overlay
    module.clearOverlays = lambda: module._registered.clear()
    return module


COIN = _build_fake_coin()


def install_fake_coin():
    """Install the stub as ``pivy.coin`` for this suite."""
    pivy = types.ModuleType("pivy")
    pivy.coin = COIN
    sys.modules["pivy"] = pivy
    sys.modules["pivy.coin"] = COIN
    return COIN


install_fake_coin()

_LOADED = None


def load_mod():
    """Load ``InitGui.py`` by path (the mod imports its stubbed ``pivy.coin``)."""
    global _LOADED
    if _LOADED is not None:
        # Another suite may have replaced ``sys.modules['pivy.coin']`` at import
        # time (the grid suite installs its own stub). The mod's classes look up
        # the module global at call time, so pin this suite's stub.
        _LOADED.coin = COIN
        _LOADED._COIN_ERROR = None
        return _LOADED
    FreeCADGui.removeCommand(COMMAND)
    spec = importlib.util.spec_from_file_location("AxisOverlayCoin_InitGui", MOD_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    module.coin = COIN
    module._COIN_ERROR = None
    _LOADED = module
    return module


def close_documents():
    for name in list(FreeCAD.listDocuments()):
        FreeCAD.closeDocument(name)


def clear_preferences():
    FreeCAD.ParamGet(PREFERENCES).Clear()
    FreeCAD.ParamGet(GRID_PREFERENCES).RemBool("GridAxisCross")
    FreeCAD.ParamGet(LEGACY_PREFERENCES).RemBool("GridAxisCross")


ATTRIBUTES = [entry["name"] for entry in _axis.SETTINGS_SCHEMA]


class AxisOverlayCoinTests(unittest.TestCase):
    def setUp(self):
        close_documents()
        self.axis = FerroCAD.Axis
        self.axis.reset()
        clear_preferences()
        for provider in FerroCADGui.providers("axis"):
            FerroCADGui.unregisterProvider("axis", provider["backend"])
        self.mod = load_mod()
        self.mod.register_provider()
        self._drop_overlay()
        # Activation runs the provider's ``activate`` (it builds and binds the
        # root), so start from a clean overlay after the switch.
        FerroCADGui.setActiveProvider("axis", "coin")
        self._drop_overlay()

    def tearDown(self):
        self._drop_overlay()
        FerroCADGui.unregisterProvider("axis", "coin")
        FreeCADGui._cancel_all()
        self.axis.reset()
        clear_preferences()
        close_documents()
        COIN._registered.clear()

    def _drop_overlay(self):
        if self.mod._overlay is not None:
            self.mod._overlay.remove()
        self.mod._overlay = None
        COIN._registered.clear()

    def spec(self):
        return FreeCADGui.runCommand(COMMAND)

    def fallback_spec(self):
        """Run the command with no registered axis provider."""
        for provider in FerroCADGui.providers("axis"):
            FerroCADGui.unregisterProvider("axis", provider["backend"])
        try:
            return FreeCADGui.runCommand(COMMAND)
        finally:
            self.mod.register_provider()

    # -- the command and the provider -----------------------------------
    def test_the_command_registers_with_the_coin_axis_surface(self):
        self.assertEqual(FreeCAD.GuiUp, 0, "this suite is headless")
        self.assertIn(COMMAND, FreeCADGui.listCommands())
        entry = next(
            item for item in FreeCADGui._command_list() if item["id"] == COMMAND
        )
        self.assertEqual(entry["menu"], "Axis Cross (Coin)")
        self.assertTrue(entry["enabled"], "IsActive() returns True")

        resources = self.mod.AxisOverlayCoinCommand().GetResources()
        self.assertEqual(resources["MenuText"], "Axis Cross (Coin)")
        self.assertEqual(resources["FerroCAD_Surface"], "axis")
        self.assertEqual(resources["FerroCAD_Parent"], "App_ToggleAxisCross")
        self.assertEqual(resources["Group"], "View")
        self.assertEqual(resources["CmdType"], "ForView")

    def test_the_provider_is_the_coin_axis_backend(self):
        self.assertEqual(FerroCADGui.activeProvider("axis"), "coin")
        entry = next(
            item
            for item in FerroCADGui.providers("axis")
            if item["backend"] == "coin"
        )
        self.assertEqual(entry["label"], "Coin (WGSL)")
        self.assertEqual(entry["priority"], 0)
        self.assertEqual(entry["command"], COMMAND)
        self.assertEqual(self.mod.settings_node(), "Settings/Axis")
        self.assertEqual(self.spec(), {"select": "Settings/Axis"})

    def test_the_provider_advertises_the_shared_schema_and_store(self):
        rows = FerroCADGui.settings("axis")
        self.assertEqual([row["name"] for row in rows], ATTRIBUTES)
        by_name = {row["name"]: row for row in rows}
        self.assertEqual(by_name["enabled"]["value"], "True")
        self.assertEqual(by_name["color_x"]["value"], "#f87171")
        self.assertEqual(by_name["length"]["value"], "80.0")
        self.assertEqual(by_name["ghost_alpha"]["value"], "0.1")

        # The rows are computed through FerroCAD.Axis: write the store and the
        # row follows, proving the two backends share one store.
        self.axis.color_y = "#010203"
        by_name = {row["name"]: row for row in FerroCADGui.settings("axis")}
        self.assertEqual(by_name["color_y"]["value"], "#010203")

        hooks = {entry["name"]: entry for entry in self.mod.settings_schema()}
        hooks["length"]["set"](120.0)
        self.assertEqual(self.axis.length, 120.0)
        self.assertAlmostEqual(hooks["opacity"]["get"](), self.axis.opacity)
        self.assertAlmostEqual(self.mod.setting("ghost_alpha"), 0.10)
        self.mod.save_setting("color_x", "#ff0000")
        self.assertEqual(self.axis.color_x, "#ff0000")

    # -- the pull bridge ------------------------------------------------
    def test_activate_registers_the_overlay_and_deactivate_removes_it(self):
        with mock.patch.object(FerroCADGui, "addOverlay") as add, \
                mock.patch.object(FerroCADGui, "removeOverlay") as remove:
            self.mod.activate()
            add.assert_called_once()
            root = add.call_args.args[0]
            self.assertIsInstance(root, COIN.SoSwitch)
            self.assertEqual(root.whichChild, COIN.SO_SWITCH_ALL)
            self.assertTrue(self.axis.enabled)

            self.mod.deactivate()
            remove.assert_called_once_with(root)
            self.assertEqual(root.whichChild, COIN.SO_SWITCH_NONE)

    def test_refresh_rebuilds_and_re_registers_the_same_root(self):
        with mock.patch.object(FerroCADGui, "addOverlay") as add, \
                mock.patch.object(FerroCADGui, "removeOverlay") as remove:
            self.mod.activate()
            root = add.call_args.args[0]
            overlay = self.mod._overlay
            add.reset_mock()
            remove.reset_mock()

            self.axis.length = 120.0
            self.axis.color_y = "#00ff00"
            self.mod.refresh()

            remove.assert_called_once_with(root)
            add.assert_called_once_with(root)
            self.assertIs(self.mod._overlay, overlay, "one reusable root")
            self.assertAlmostEqual(overlay.axis_length.value, 120.0)
            self.assertEqual(overlay.axis_color_y.value, (0.0, 1.0, 0.0))
            params = {p.name: p for p in overlay.fragment.parameter.values}
            self.assertAlmostEqual(params["axis_length"].value, 120.0)

    def test_the_tree_carries_the_expected_parameters_and_nodes(self):
        overlay = self.mod.ShaderAxis()
        try:
            self.assertEqual(overlay.vertex.source, MOD_WGSL.read_text(encoding="utf-8"))
            self.assertEqual(overlay.fragment.source, "", "WGSL has no per-stage modules")

            names = {parameter.name for parameter in overlay.parameters}
            self.assertEqual(
                names,
                {
                    "grid_color",
                    "axis_color_y",
                    "axis_color_z",
                    "axis_length",
                    "axis_width",
                    "axis_opacity",
                    "axis_ghost_alpha",
                    "axis_origin",
                    "axis_labels",
                },
            )

            self.assertEqual(len(overlay.root.children), 3, "pick, light, separator")
            pick, light, _separator = overlay.root.children
            self.assertIsInstance(pick, COIN.SoPickStyle)
            self.assertEqual(pick.style, COIN.SoPickStyle.UNPICKABLE)
            self.assertIsInstance(light, COIN.SoLightModel)
            self.assertEqual(light.model, COIN.SoLightModel.BASE_COLOR)

            sep = overlay.sep
            self.assertEqual(sep.renderCulling, COIN.SoSeparator.OFF)
            self.assertIn(overlay.depth, sep.children)
            self.assertIn(overlay.program, sep.children)
            self.assertIn(overlay.coords, sep.children)
            self.assertIn(overlay.face, sep.children)
            self.assertIsInstance(overlay.depth, COIN.SoDepthBuffer)
            self.assertTrue(overlay.depth.test)
            self.assertFalse(overlay.depth.write)
            self.assertEqual(overlay.depth.function, COIN.SoDepthBuffer.LESS)
        finally:
            overlay.remove()

    def test_the_wgsl_copy_is_byte_identical_to_the_crate_reference(self):
        self.assertEqual(
            MOD_WGSL.read_bytes(),
            CRATE_WGSL.read_bytes(),
            "mods/AxisOverlayCoin/Resources/axis_overlay.wgsl must mirror the crate",
        )

    # -- the fallback dialog and upstream paths -------------------------
    def test_the_fallback_dialog_covers_the_schema_and_writes_the_axis(self):
        spec = self.fallback_spec()
        self.assertNotIn("select", spec)
        self.assertEqual(spec["title"], "Axis Cross (Coin) Settings")
        self.assertEqual(spec["accept"], "Apply")
        self.assertTrue(callable(spec["on_accept"]))
        fields = {field["name"]: field for field in spec["fields"]}
        self.assertEqual(list(fields), ATTRIBUTES)
        self.assertEqual(fields["enabled"]["kind"], "bool")
        self.assertEqual(fields["color_x"]["kind"], "text")
        self.assertEqual(fields["length"]["kind"], "float")
        self.assertEqual(fields["length"]["min"], 8.0)
        self.assertEqual(fields["length"]["max"], 400.0)

        result = spec["on_accept"](
            {
                "enabled": False,
                "color_x": "#FF0000",
                "opacity": 0.5,
                "length": 120.0,
                "width": 2.5,
                "ghost_alpha": 0.2,
            }
        )
        self.assertEqual(result, self.axis.apply())
        self.assertFalse(self.axis.enabled)
        self.assertEqual(self.axis.color_x, "#ff0000")
        self.assertEqual(self.axis.opacity, 0.5)
        self.assertEqual(self.axis.length, 120.0)
        self.assertEqual(self.axis.width, 2.5)
        self.assertEqual(self.axis.ghost_alpha, 0.2)

    def test_the_fallback_dialog_reports_without_the_axis_api(self):
        for provider in FerroCADGui.providers("axis"):
            FerroCADGui.unregisterProvider("axis", provider["backend"])
        try:
            with mock.patch.object(FerroCAD, "Axis", None):
                spec = self.spec()
        finally:
            self.mod.register_provider()
        self.assertEqual(spec, {"status": self.mod.UPSTREAM_STATUS})

    def test_a_build_without_the_bridge_reports_instead_of_crashing(self):
        with mock.patch.object(self.mod, "overlay_api", return_value=None):
            result = self.fallback_spec()
        self.assertIn("status", result)
        self.assertIn("addOverlay", result["status"])
        self.assertNotIn("fields", result)


if __name__ == "__main__":
    unittest.main()
