"""Tests for the GridOverlayCoin mod (G5, the A1 Coin/shader fork).

The mod is the upstream add-on's Coin tree carried by FerroCAD's **pull** overlay
bridge: enabling calls ``FreeCADGui.addOverlay(root)``, disabling removes it, and
a parameter change re-binds (``_rebind``). The WGSL file in the mod is a duplicate
of the crate's reference copy and must stay byte-identical.

Headless (``FreeCAD.GuiUp == 0``): the test installs a small ``pivy.coin`` stub so
the mod can build its tree without the Rust extension, then spies on the facade's
overlay functions. Nothing here needs a GPU. See ``docs/grid-overlay-plan.md``
§3.2, §5 G5.
"""

import importlib.util
import pathlib
import sys
import types
import unittest
from unittest import mock

import FreeCAD
import FreeCADGui

MOD_DIR = (
    pathlib.Path(__file__).resolve().parents[1] / "mods" / "GridOverlayCoin"
)
MOD_PATH = MOD_DIR / "InitGui.py"
MOD_WGSL = MOD_DIR / "Resources" / "grid_overlay.wgsl"
CRATE_WGSL = (
    pathlib.Path(__file__).resolve().parents[1]
    / "crates"
    / "ferrocad_wgpu"
    / "shaders"
    / "grid_overlay.wgsl"
)
COMMAND = "GridOverlayCoin_Settings"
KNOBS = {
    "enabled",
    "color",
    "opacity",
    "base_spacing",
    "shadow_enabled",
    "shadow_opacity",
}


# ---------------------------------------------------------------------------
# A minimal ``pivy.coin`` stub: fields, groups and the constants a Coin tree uses
# ---------------------------------------------------------------------------


class _FakeField:
    """A Coin field with the arities the add-on uses."""

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
        self.transparency = _FakeField()
        self.diffuseColor = _FakeField()
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
        setattr(self, field, value.strip().lower() in ("true", "1", "yes", "on"))


def _fake_type(name, **attributes):
    return type(name, (_FakeNode,), attributes)


class _FakeFloatArrayNode(_FakeNode):
    """``SoShaderParameterArray1f``: its ``value`` is an ``SoMFFloat``."""

    def __init__(self):
        super().__init__()
        self.value = _FakeField()


def _build_fake_coin():
    module = types.ModuleType("pivy.coin")
    module.SO_SWITCH_NONE = -1
    module.SO_SWITCH_INHERIT = -2
    module.SO_SWITCH_ALL = -3
    module.SoSeparator = _fake_type("SoSeparator", OFF=0, AUTO=1, ON=2)
    module.SoSwitch = _fake_type("SoSwitch")
    module.SoDepthBuffer = _fake_type("SoDepthBuffer")
    module.SoVertexShader = _fake_type("SoVertexShader")
    module.SoFragmentShader = _fake_type("SoFragmentShader")
    module.SoShaderProgram = _fake_type("SoShaderProgram")
    module.SoCoordinate3 = _fake_type("SoCoordinate3")
    module.SoFaceSet = _fake_type("SoFaceSet")
    module.SoMaterial = _fake_type("SoMaterial")
    module.SoPickStyle = _fake_type("SoPickStyle", UNPICKABLE=0, SHAPE=1)
    module.SoLightModel = _fake_type("SoLightModel", BASE_COLOR=0, PHONG=1)
    module.SoShaderParameter1f = _fake_type("SoShaderParameter1f")
    module.SoShaderParameter3f = _fake_type("SoShaderParameter3f")
    module.SoShaderParameterArray1f = _FakeFloatArrayNode
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
    """Make ``from pivy import coin`` resolve to the stub, once per process."""
    if "pivy.coin" in sys.modules:
        return sys.modules["pivy.coin"]
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
        return _LOADED
    FreeCADGui.removeCommand(COMMAND)
    spec = importlib.util.spec_from_file_location("GridOverlayCoin_InitGui", MOD_PATH)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    _LOADED = module
    return module


def close_documents():
    for name in list(FreeCAD.listDocuments()):
        FreeCAD.closeDocument(name)


class GridOverlayCoinTests(unittest.TestCase):
    def setUp(self):
        close_documents()
        self.mod = load_mod()
        self.mod.SETTINGS.Clear()
        self.mod.manager.remove_grid()
        self.mod.manager.items.clear()
        COIN._registered.clear()

    def tearDown(self):
        self.mod.manager.remove_grid()
        self.mod.manager.items.clear()
        self.mod.SETTINGS.Clear()
        COIN._registered.clear()
        FreeCADGui._cancel_all()
        close_documents()

    def spec(self):
        return FreeCADGui.runCommand(COMMAND)

    # -- the command and its dialog -------------------------------------
    def test_the_command_registers_with_the_coin_menu_text(self):
        self.assertEqual(FreeCAD.GuiUp, 0, "this suite is headless")
        self.assertIn(COMMAND, FreeCADGui.listCommands())
        entry = next(
            item for item in FreeCADGui._command_list() if item["id"] == COMMAND
        )
        self.assertEqual(entry["menu"], "Grid Overlay (Coin)")
        self.assertTrue(entry["enabled"], "IsActive() returns True")

        resources = self.mod.GridOverlayCoinCommand().GetResources()
        self.assertEqual(resources["MenuText"], "Grid Overlay (Coin)")
        self.assertTrue(resources["Pixmap"].endswith("grid.svg"))

    def test_activated_returns_the_six_field_dialog(self):
        spec = self.spec()
        self.assertEqual(spec["title"], "Grid Overlay (Coin) Settings")
        self.assertEqual(spec["accept"], "Apply")
        self.assertTrue(callable(spec["on_accept"]))

        fields = {field["name"]: field for field in spec["fields"]}
        self.assertEqual(set(fields), KNOBS)
        self.assertEqual(fields["enabled"]["value"], True)
        self.assertEqual(fields["color"]["value"], "#808080")
        self.assertAlmostEqual(fields["opacity"]["value"], 0.65)
        self.assertAlmostEqual(fields["base_spacing"]["value"], 1.0)
        self.assertEqual(fields["shadow_enabled"]["value"], True)
        self.assertAlmostEqual(fields["shadow_opacity"]["value"], 0.30)

    def test_the_dialog_seeds_from_the_live_settings(self):
        self.mod.SETTINGS.SetBool("enabled", False)
        self.mod.SETTINGS.SetString("color", "#123456")
        self.mod.SETTINGS.SetFloat("opacity", 0.5)
        self.mod.SETTINGS.SetFloat("base_spacing", 2.0)
        self.mod.SETTINGS.SetBool("shadow_enabled", False)
        self.mod.SETTINGS.SetFloat("shadow_opacity", 0.25)

        fields = {field["name"]: field["value"] for field in self.spec()["fields"]}
        self.assertEqual(
            fields,
            {
                "enabled": False,
                "color": "#123456",
                "opacity": 0.5,
                "base_spacing": 2.0,
                "shadow_enabled": False,
                "shadow_opacity": 0.25,
            },
        )

    def test_on_accept_persists_and_returns_the_overlay_view(self):
        result = self.spec()["on_accept"](
            {
                "enabled": True,
                "color": "#f87171",
                "opacity": 0.5,
                "base_spacing": 2.0,
                "shadow_enabled": True,
                "shadow_opacity": 0.25,
            }
        )

        self.assertEqual(result, {"view": {"grid_overlay": True}})
        self.assertTrue(self.mod.setting("enabled"))
        self.assertEqual(self.mod.setting("color"), "#f87171")
        self.assertAlmostEqual(self.mod.setting("opacity"), 0.5)
        self.assertAlmostEqual(self.mod.setting("base_spacing"), 2.0)
        self.assertTrue(self.mod.setting("shadow_enabled"))
        self.assertAlmostEqual(self.mod.setting("shadow_opacity"), 0.25)

    def test_a_bad_color_raises_and_leaves_the_store_alone(self):
        with self.assertRaises(ValueError):
            self.spec()["on_accept"]({"color": "not a colour"})
        self.assertEqual(self.mod.setting("color"), "#808080")

    def test_a_build_without_the_bridge_reports_instead_of_crashing(self):
        with mock.patch.object(self.mod, "overlay_api", return_value=None):
            result = self.mod.GridOverlayCoinCommand().Activated()
        self.assertIn("status", result)
        self.assertIn("addOverlay", result["status"])
        self.assertNotIn("fields", result)

    # -- the pull bridge ------------------------------------------------
    def test_enabling_registers_a_coin_switch_and_disabling_removes_it(self):
        grid = self.mod.ShaderGrid("Doc")
        with mock.patch.object(FreeCADGui, "addOverlay") as add, \
                mock.patch.object(FreeCADGui, "removeOverlay") as remove:
            grid.set_visible(True)
            add.assert_called_once()
            root = add.call_args.args[0]
            self.assertIsInstance(root, COIN.SoSwitch)
            self.assertEqual(root.whichChild, COIN.SO_SWITCH_ALL)
            self.assertEqual(len(root.children), 3, "pick, light, separator")

            grid.set_visible(False)
            remove.assert_called_once_with(root)
            self.assertEqual(root.whichChild, COIN.SO_SWITCH_NONE)

    def test_rebind_removes_and_re_adds_the_same_root(self):
        grid = self.mod.ShaderGrid("Doc")
        with mock.patch.object(FreeCADGui, "addOverlay") as add, \
                mock.patch.object(FreeCADGui, "removeOverlay") as remove:
            grid.set_visible(True)
            add.reset_mock()
            remove.reset_mock()

            self.mod.SETTINGS.SetFloat("base_spacing", 2.0)
            self.mod.SETTINGS.SetString("color", "#ff0000")
            grid.apply_settings()

            remove.assert_called_once_with(grid.root)
            add.assert_called_once_with(grid.root)
            self.assertAlmostEqual(grid.base_spacing.value, 2.0)
            self.assertEqual(grid.grid_color.value, (1.0, 0.0, 0.0))

            # The rebind wrote the new value into the stage's value-copied list.
            params = {p.name: p for p in grid.fragment.parameter.values}
            self.assertAlmostEqual(params["base_spacing"].value, 2.0)

    def test_the_tree_carries_the_ported_wgsl_on_one_stage(self):
        grid = self.mod.ShaderGrid("Doc")
        self.assertEqual(grid.vertex.source, MOD_WGSL.read_text(encoding="utf-8"))
        self.assertEqual(grid.fragment.source, "", "WGSL has no per-stage modules")
        # Every parameter the fixed ABI names is present and typed.
        names = {parameter.name for parameter in grid.parameters}
        self.assertTrue(
            {"grid_color", "base_spacing", "minor_alpha", "major_alpha",
             "shadow_opacity", "shadow_count"} <= names
        )
        self.assertIn("shadow_data", names, "the deferred block is still carried")

    def test_the_mod_uses_no_qt_and_no_live_scene_graph(self):
        for name in ("QtCore", "QtGui", "QtWidgets", "QDialog", "QSettings"):
            self.assertFalse(hasattr(self.mod, name), name)
        self.assertFalse(hasattr(self.mod.ShaderGrid, "getSceneGraph"))
        self.assertFalse(hasattr(self.mod.GridManager, "getMainWindow"))

    # -- per document ---------------------------------------------------
    def test_only_the_active_document_keeps_an_overlay(self):
        with mock.patch.object(FreeCADGui, "addOverlay") as add, \
                mock.patch.object(FreeCADGui, "removeOverlay") as remove:
            FreeCAD.newDocument("G5DocA")
            FreeCAD.setActiveDocument("G5DocA")
            self.mod.manager.ensure_grid()
            self.assertIn("G5DocA", self.mod.manager.items)
            first = self.mod.manager.items["G5DocA"]
            self.assertTrue(first._registered)

            FreeCAD.newDocument("G5DocB")
            FreeCAD.setActiveDocument("G5DocB")
            self.mod.manager.ensure_grid()
            self.assertIn("G5DocB", self.mod.manager.items)
            second = self.mod.manager.items["G5DocB"]
            self.assertTrue(second._registered)
            self.assertFalse(
                first._registered,
                "only the active document's grid is bound to the viewport",
            )

            FreeCAD.closeDocument("G5DocB")
            self.assertNotIn("G5DocB", self.mod.manager.items)
            self.assertGreaterEqual(add.call_count, 2)
            self.assertGreaterEqual(remove.call_count, 1)

    # -- the duplicated shader -----------------------------------------
    def test_the_wgsl_copy_is_byte_identical_to_the_crate_reference(self):
        self.assertEqual(
            MOD_WGSL.read_bytes(),
            CRATE_WGSL.read_bytes(),
            "mods/GridOverlayCoin/Resources/grid_overlay.wgsl must mirror the crate",
        )


if __name__ == "__main__":
    unittest.main()
