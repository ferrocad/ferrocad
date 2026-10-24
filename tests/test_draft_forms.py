"""Q5: headless accept/reject paths for the four remaining Draft forms.

The extension is loaded from the cargo target directory explicitly, exactly as
``tests/test_ui_loader.py`` does, so a raw ``cargo build`` artifact works. The
whole class is skipped when the proxy, the native ``ferrocad`` extension or the
upstream Draft ``.ui`` resources are absent.

No test imports PySide and none renders a widget: each test drives the
``FormProxy`` model directly through ``FerroCADGui.forms.open_panel``.
"""

from __future__ import annotations

import importlib.machinery
import importlib.util
import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
PYTHON_DIR = REPO / "python"
UPSTREAM = Path(os.environ.get("FERROCAD_UPSTREAM", REPO.parent / "freecad-upstream"))
UI_DIR = UPSTREAM / "src" / "Mod" / "Draft" / "Resources" / "ui"


def _library_candidates():
    override = os.environ.get("FERROCAD_UI_PY_LIB")
    if override:
        yield Path(override)
    for profile in ("debug", "release"):
        directory = REPO / "target" / profile
        yield directory / "ferrocad_ui_py.abi3.so"
        yield directory / "ferrocad_ui_py.so"
        yield directory / "libferrocad_ui_py.so"
        yield directory / "libferrocad_ui_py.dylib"
        yield directory / "ferrocad_ui_py.pyd"
        yield directory / "libferrocad_ui_py.pyd"
    yield PYTHON_DIR / "ferrocad_ui_py.abi3.so"


def _load_extension():
    """Import ``ferrocad_ui_py``, falling back to a raw cargo cdylib."""
    try:
        import ferrocad_ui_py  # noqa: F401 - the import is the test

        return ferrocad_ui_py
    except ImportError:
        pass
    for candidate in _library_candidates():
        if not candidate.is_file():
            continue
        loader = importlib.machinery.ExtensionFileLoader(
            "ferrocad_ui_py", str(candidate)
        )
        spec = importlib.util.spec_from_file_location(
            "ferrocad_ui_py", str(candidate), loader=loader
        )
        module = importlib.util.module_from_spec(spec)
        sys.modules["ferrocad_ui_py"] = module
        try:
            loader.exec_module(module)
        except BaseException:
            sys.modules.pop("ferrocad_ui_py", None)
            raise
        return module
    return None


UI_EXTENSION = _load_extension()

if str(PYTHON_DIR) not in sys.path:
    sys.path.insert(0, str(PYTHON_DIR))

# Importing ``forms`` registers the four commands; it does not load a form.
try:
    import FreeCAD
    import FreeCADGui
    from FerroCADGui import forms
except ImportError:  # pragma: no cover - covered by the skip below
    FreeCAD = FreeCADGui = forms = None


def _ferrocad_extension_built() -> bool:
    return any(PYTHON_DIR.glob("ferrocad*.so")) or any(
        PYTHON_DIR.glob("ferrocad*.pyd")
    )


@unittest.skipUnless(
    UI_DIR.is_dir(), f"upstream Draft .ui files are not available at {UI_DIR}"
)
@unittest.skipUnless(
    UI_EXTENSION is not None,
    "ferrocad_ui_py is not built; run `cargo build -p ferrocad_ui_py --offline`",
)
@unittest.skipUnless(
    _ferrocad_extension_built(),
    f"the ferrocad extension is not built in {PYTHON_DIR}",
)
@unittest.skipUnless(forms is not None, "FerroCADGui.forms could not be imported")
class DraftFormsTests(unittest.TestCase):
    """The four Q5 commands, their fields, and their accept/reject paths."""

    def setUp(self):
        from FerroCADGui.forms import DRAFT_PREFERENCES

        self.group = FreeCAD.ParamGet(DRAFT_PREFERENCES)
        # Nothing else in the process uses the Draft group; clearing it makes
        # every test independent of the ones before it.
        self.group.Clear()
        FreeCADGui.Selection._set([])
        FreeCADGui._cancel_all()
        self.doc = FreeCAD.newDocument("DraftForms")
        self.doc_name = self.doc.Name

    def tearDown(self):
        FreeCADGui.Selection._set([])
        FreeCADGui._cancel_all()
        try:
            FreeCAD.closeDocument(self.doc_name)
        except Exception:  # noqa: BLE001 - already closed by a failing test
            pass
        self.group.Clear()

    # -- helpers -----------------------------------------------------------

    def _pat_file(self, directory) -> str:
        path = Path(directory) / "test.pat"
        path.write_text(
            "*SOLID, Solid fill\n"
            "0, 0,0, 0,0\n"
            "*ANSI31, Iron\n"
            "45, 0,0, 0,0.125\n",
            encoding="utf-8",
        )
        return str(path)

    def _hatch_object(self):
        obj = self.doc.addObject("App::Link", "HatchTarget")
        obj.addProperty("App::PropertyString", "File")
        obj.addProperty("App::PropertyString", "Pattern")
        obj.addProperty("App::PropertyFloat", "Rotation")
        obj.addProperty("App::PropertyBool", "Translate")
        obj.File = ""
        obj.Pattern = ""
        obj.Scale = 1.0
        return obj

    # -- registration and the dialog spec ---------------------------------

    def test_all_four_commands_are_registered_and_payload_compatible(self):
        for name in forms.COMMANDS:
            with self.subTest(command=name):
                self.assertIn(name, FreeCADGui.listCommands())

                payload = FreeCADGui._run(name)
                self.assertTrue(payload["ok"], payload)
                dialog = payload["dialog"]
                self.assertEqual(dialog["accept"], "OK")
                self.assertEqual(dialog["cancel"], "Cancel")
                self.assertTrue(dialog["title"])
                self.assertTrue(dialog["fields"])
                # `_payload` kept the callback under a single-use token.
                self.assertEqual(FreeCADGui._cancel(payload["token"]), {"ok": True})

        listed = {entry["id"]: entry for entry in FreeCADGui._command_list()}
        for name in forms.COMMANDS:
            self.assertTrue(listed[name]["enabled"])
            self.assertTrue(listed[name]["menu"])

    def test_ui_path_resolution_is_explicit_and_fails_clearly(self):
        hatch = forms.ui_path("Draft_Hatch")
        self.assertEqual(hatch.name, "dialogHatch.ui")
        self.assertTrue(hatch.is_file())

        scale = forms.ui_path("Draft_Scale")
        self.assertEqual(scale.name, "TaskPanel_Scale.ui")
        self.assertEqual(
            scale.parent,
            PYTHON_DIR / "FerroCADGui" / "Resources" / "ui",
        )

        with tempfile.TemporaryDirectory() as directory:
            # An explicit directory wins, but a missing form is an error.
            with self.assertRaises(FileNotFoundError):
                forms.ui_path("Draft_Hatch", directory)
            copied = Path(directory) / "dialogHatch.ui"
            copied.write_text(hatch.read_text(encoding="utf-8"), encoding="utf-8")
            self.assertEqual(
                forms.ui_path("Draft_Hatch", directory), copied
            )

        with self.assertRaises(FileNotFoundError):
            forms.ui_directory("/definitely/not/a/draft/checkout")

    # -- Draft_SetStyle ----------------------------------------------------

    def test_setstyle_loads_the_widgets_and_accepts_into_parameters(self):
        panel = forms.open_panel("Draft_SetStyle")
        for widget in (
            "ShapeColor",
            "AmbientColor",
            "LineColor",
            "LineWidth",
            "TextColor",
            "TextFont",
            "TextSize",
            "LineSpacing",
            "TextSpacing",
            "DisplayMode",
            "DrawStyle",
            "ShowUnit",
            "comboPresets",
            "saveButton",
            "applyButton",
            "annotButton",
            "tabs",
            "tab_Shape",
            "tab_Annotation",
        ):
            self.assertEqual(
                getattr(panel.form, widget).objectName(), widget
            )

        panel.form.TextSize.setProperty("rawValue", 7.25)
        panel.form.ShapeColor.setProperty("color", (255, 0, 0, 255))
        panel.form.ShowUnit.setChecked(True)
        # The proxy models a font combo, not font enumeration.
        panel.form.TextFont.addItem("DejaVu Sans")
        panel.form.TextFont.setCurrentText("DejaVu Sans")

        result = panel.accept()
        self.assertIn("29 values", result["status"])
        self.assertEqual(result["value"]["TextSize"], 7.25)

        self.assertAlmostEqual(self.group.GetFloat("textheight", 0.0), 7.25)
        self.assertEqual(
            self.group.GetUnsigned("DefaultShapeColor", 0), 0xFF0000FF
        )
        self.assertEqual(self.group.GetString("textfont", ""), "DejaVu Sans")
        self.assertTrue(self.group.GetBool("showUnit", False))

        # A fresh panel seeds from ParamGet, which is the round-trip that matters.
        again = forms.open_panel("Draft_SetStyle")
        self.assertAlmostEqual(again.form.TextSize.property("rawValue"), 7.25)
        self.assertEqual(again.form.TextFont.currentText(), "DejaVu Sans")
        self.assertTrue(again.form.ShowUnit.isChecked())
        self.assertEqual(
            tuple(again.form.ShapeColor.property("color")), (255, 0, 0, 255)
        )

    def test_setstyle_presets_round_trip_through_paramget(self):
        panel = forms.open_panel("Draft_SetStyle")
        panel.form.TextSize.setProperty("rawValue", 6.5)
        panel.form.LineWidth.setValue(4)
        panel.form.ShapeColor.setProperty("color", (0, 200, 0, 255))
        saved = panel.save_preset("RoundTrip")

        self.assertAlmostEqual(saved["TextSize"], 6.5)
        self.assertEqual(saved["LineWidth"], 4)
        self.assertIn("RoundTrip", panel.presets())

        raw = self.group.GetString(forms.PRESET_KEY, "")
        self.assertIn("RoundTrip", json.loads(raw))

        panel.form.TextSize.setProperty("rawValue", 1.0)
        panel.form.LineWidth.setValue(1)
        panel.load_preset("RoundTrip")
        self.assertAlmostEqual(panel.form.TextSize.property("rawValue"), 6.5)
        self.assertEqual(panel.form.LineWidth.value(), 4)

        # A new controller reads the presets from ParamGet, not from memory.
        fresh = forms.open_panel("Draft_SetStyle")
        self.assertIn("RoundTrip", fresh.presets())
        fresh.select_preset("RoundTrip")
        self.assertAlmostEqual(fresh.form.TextSize.property("rawValue"), 6.5)
        self.assertEqual(fresh.values()["ShapeColor"], "#00c800ff")

        self.assertTrue(panel.delete_preset("RoundTrip"))
        self.assertNotIn("RoundTrip", panel.presets())

    def test_setstyle_reject_writes_nothing(self):
        panel = forms.open_panel("Draft_SetStyle")
        panel.form.TextSize.setProperty("rawValue", 9.75)
        panel.form.ShapeColor.setProperty("color", (1, 2, 3, 255))
        result = panel.reject()

        self.assertIn("cancelled", result["status"])
        self.assertNotIn("textheight", self.group.GetFloats())
        self.assertNotIn("DefaultShapeColor", self.group.GetUnsigneds())
        self.assertEqual(self.group.GetString(forms.PRESET_KEY, ""), "")

    # -- Draft_Hatch -------------------------------------------------------

    def test_hatch_file_chooser_loads_patterns_and_saves_parameters(self):
        with tempfile.TemporaryDirectory() as directory:
            filename = self._pat_file(directory)
            panel = forms.open_panel("Draft_Hatch")
            self.assertEqual(panel.form.File.objectName(), "File")
            self.assertEqual(panel.form.Pattern.objectName(), "Pattern")

            # Gui::FileChooser.fileName round-trips, and the form's
            # fileNameChanged callback fills the Pattern combo.
            self.assertEqual(panel.form.File.fileName(), "")
            panel.form.File.setFileName(filename)
            self.assertEqual(panel.form.File.fileName(), filename)
            self.assertEqual(panel.form.File.property("fileName"), filename)
            self.assertEqual(
                [panel.form.Pattern.itemText(index)
                 for index in range(panel.form.Pattern.count())],
                ["SOLID", "ANSI31"],
            )
            self.assertEqual(panel.patterns(), ["SOLID", "ANSI31"])

            panel.form.Pattern.setCurrentText("ANSI31")
            panel.form.Scale.setValue(250.0)
            panel.form.Rotation.setValue(30.0)
            panel.form.Translate.setChecked(True)
            result = panel.accept()

            # The next panel seeds the stored file and pattern while the file
            # still exists on disk.
            again = forms.open_panel("Draft_Hatch")
            self.assertEqual(again.form.File.fileName(), filename)
            self.assertEqual(
                [again.form.Pattern.itemText(index)
                 for index in range(again.form.Pattern.count())],
                ["SOLID", "ANSI31"],
            )
            self.assertEqual(again.form.Pattern.currentText(), "ANSI31")

        self.assertIn("no supported object selected", result["status"])
        self.assertEqual(result["value"]["Pattern"], "ANSI31")
        self.assertEqual(self.group.GetString("HatchPatternFile", ""), filename)
        self.assertEqual(self.group.GetString("HatchPatternName", ""), "ANSI31")
        self.assertAlmostEqual(self.group.GetFloat("HatchPatternScale", 0.0), 250.0)
        self.assertAlmostEqual(
            self.group.GetFloat("HatchPatternRotation", 0.0), 30.0
        )
        self.assertTrue(self.group.GetBool("HatchPatternTranslate", False))

    def test_hatch_accept_applies_to_a_supported_selection(self):
        obj = self._hatch_object()
        FreeCADGui.Selection._set([(obj.Name, [])])

        with tempfile.TemporaryDirectory() as directory:
            filename = self._pat_file(directory)
            panel = forms.open_panel("Draft_Hatch")
            panel.form.File.setFileName(filename)
            panel.form.Pattern.setCurrentText("SOLID")
            panel.form.Scale.setValue(500.0)
            panel.form.Rotation.setValue(15.0)
            panel.form.Translate.setChecked(True)
            result = panel.accept()

        self.assertIn(obj.Name, result["status"])
        self.assertEqual(result["value"]["applied"], [obj.Name])
        self.assertEqual(obj.File, filename)
        self.assertEqual(obj.Pattern, "SOLID")
        self.assertAlmostEqual(obj.Scale, 500.0)
        self.assertAlmostEqual(obj.Rotation, 15.0)
        self.assertTrue(obj.Translate)

    def test_hatch_reject_writes_nothing(self):
        obj = self._hatch_object()
        obj.File = "before.pat"
        obj.Pattern = "BEFORE"
        obj.Scale = 1.0
        FreeCADGui.Selection._set([(obj.Name, [])])

        panel = forms.open_panel("Draft_Hatch")
        panel.form.File.setFileName("after.pat")
        panel.form.Scale.setValue(999.0)
        result = panel.reject()

        self.assertIn("cancelled", result["status"])
        self.assertEqual(obj.File, "before.pat")
        self.assertEqual(obj.Pattern, "BEFORE")
        self.assertAlmostEqual(obj.Scale, 1.0)
        self.assertNotIn("HatchPatternFile", self.group.GetStrings())

    # -- Draft_AnnotationStyleEditor --------------------------------------

    def test_annotation_editor_writes_parameters_and_document_styles(self):
        panel = forms.open_panel("Draft_AnnotationStyleEditor")
        for widget in (
            "comboBoxStyles",
            "pushButtonRename",
            "pushButtonDelete",
            "scrollArea",
            "ScaleMultiplier",
            "FontName",
            "FontSize",
            "LineSpacing",
            "LineWidth",
            "TextColor",
            "ShowLine",
            "ShowUnit",
            "UnitOverride",
            "buttonBox",
        ):
            self.assertEqual(
                getattr(panel.form, widget).objectName(), widget
            )

        panel.form.FontSize.setValue(5.5)
        panel.form.ScaleMultiplier.setValue(2.0)
        panel.form.LineWidth.setValue(3)
        panel.form.FontName.addItem("Mono")
        panel.form.FontName.setCurrentText("Mono")
        accepted = []
        panel.form.accepted.connect(lambda: accepted.append(True))

        result = panel.accept()
        self.assertEqual(accepted, [True])
        self.assertEqual(panel.form.result(), 1)
        self.assertIn("no named style was selected", result["status"])
        self.assertTrue(result["value"]["document"])
        self.assertAlmostEqual(self.group.GetFloat("textheight", 0.0), 5.5)
        self.assertAlmostEqual(
            self.group.GetFloat("DefaultAnnoScaleMultiplier", 0.0), 2.0
        )
        self.assertEqual(self.group.GetInt("DefaultAnnoLineWidth", 0), 3)
        self.assertEqual(self.group.GetString("textfont", ""), "Mono")

        # A named style lives under Draft_Style_<name> in Doc.Meta.
        panel.save_style("MetaStyle")
        self.assertIn("Draft_Style_MetaStyle", self.doc.Meta)
        stored = json.loads(self.doc.Meta["Draft_Style_MetaStyle"])
        self.assertAlmostEqual(stored["FontSize"], 5.5)
        self.assertEqual(stored["FontName"], "Mono")

        fresh = forms.open_panel("Draft_AnnotationStyleEditor")
        self.assertIn("MetaStyle", fresh.style_names())
        fresh.select_style("MetaStyle")
        self.assertAlmostEqual(fresh.form.FontSize.value(), 5.5)
        self.assertEqual(fresh.form.FontName.currentText(), "Mono")

        fresh.form.FontSize.setValue(9.0)
        accepted = fresh.accept()
        self.assertIn("'MetaStyle' updated", accepted["status"])
        stored = json.loads(self.doc.Meta["Draft_Style_MetaStyle"])
        self.assertAlmostEqual(stored["FontSize"], 9.0)
        self.assertAlmostEqual(self.group.GetFloat("textheight", 0.0), 9.0)

    def test_annotation_editor_reject_writes_nothing(self):
        self.doc.Meta = {"Unrelated": "kept"}
        panel = forms.open_panel("Draft_AnnotationStyleEditor")
        panel.form.FontSize.setValue(12.5)
        panel.form.FontName.addItem("Mono")
        panel.form.FontName.setCurrentText("Mono")
        rejected = []
        panel.form.rejected.connect(lambda: rejected.append(True))

        result = panel.reject()
        self.assertEqual(rejected, [True])
        self.assertEqual(panel.form.result(), 0)
        self.assertIn("cancelled", result["status"])
        self.assertEqual(self.doc.Meta, {"Unrelated": "kept"})
        self.assertNotIn("textheight", self.group.GetFloats())
        self.assertNotIn("Draft_Style_Unused", self.doc.Meta)

    # -- Draft_Scale -------------------------------------------------------

    def test_scale_form_members_and_accept_applies_to_an_app_link(self):
        obj = self.doc.addObject("App::Link", "ScaleTarget")
        FreeCADGui.Selection._set([(obj.Name, [])])

        panel = forms.open_panel("Draft_Scale")
        for widget in ("xValue", "yValue", "zValue", "lock", "isCopy", "buttonBox"):
            self.assertEqual(
                getattr(panel.form, widget).objectName(), widget
            )

        panel.form.xValue.setValue(2.0)
        panel.form.lock.setChecked(True)
        result = panel.accept()

        self.assertEqual(result["value"]["x"], 2.0)
        self.assertEqual(result["value"]["y"], 2.0)
        self.assertEqual(result["value"]["z"], 2.0)
        self.assertTrue(result["value"]["uniform"])
        self.assertEqual(result["value"]["applied"], [obj.Name])

        vector = obj.ScaleVector
        self.assertEqual((vector.x, vector.y, vector.z), (2.0, 2.0, 2.0))
        self.assertAlmostEqual(obj.Scale, 2.0)
        self.assertTrue(self.group.GetBool("ScaleUniform", False))

        # Non-uniform factors keep the vector's three components.
        panel = forms.open_panel("Draft_Scale")
        panel.form.lock.setChecked(False)
        panel.form.xValue.setValue(3.0)
        panel.form.yValue.setValue(4.0)
        panel.form.zValue.setValue(5.0)
        panel.accept()
        vector = obj.ScaleVector
        self.assertEqual((vector.x, vector.y, vector.z), (3.0, 4.0, 5.0))

    def test_scale_reject_writes_nothing(self):
        obj = self.doc.addObject("App::Link", "ScaleReject")
        FreeCADGui.Selection._set([(obj.Name, [])])
        before = obj.ScaleVector
        before = (before.x, before.y, before.z)

        panel = forms.open_panel("Draft_Scale")
        panel.form.xValue.setValue(9.0)
        panel.form.lock.setChecked(True)
        result = panel.reject()

        self.assertIn("cancelled", result["status"])
        after = obj.ScaleVector
        self.assertEqual((after.x, after.y, after.z), before)
        self.assertNotIn("ScaleUniform", self.group.GetBools())


if __name__ == "__main__":
    unittest.main()
