"""Q2a/Q2b: ``FreeCADGui.PySideUic.loadUi`` and the headless form proxies.

The extension is loaded from the cargo target directory explicitly. Cargo emits
``libferrocad_ui_py.so`` on Linux, while Python imports the module under the
name ``ferrocad_ui_py``; the helper below loads the library into ``sys.modules``
under that name. This makes the test work both with a raw cargo build (the
``target/debug`` artifact) and with a packaged ``ferrocad_ui_py.abi3.so``.

The six core Draft forms are read from the pinned upstream checkout; the whole
class is skipped when it or the extension is absent. No test imports PySide:
forms are loaded from filesystem paths, and the "does not import PySide" check
runs in a fresh subprocess so the rest of the suite cannot pollute it.
"""

from __future__ import annotations

import importlib.machinery
import importlib.util
import json
import os
import subprocess
import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
PYTHON_DIR = REPO / "python"
UPSTREAM = Path(os.environ.get("FERROCAD_UPSTREAM", REPO.parent / "freecad-upstream"))
UI_DIR = UPSTREAM / "src" / "Mod" / "Draft" / "Resources" / "ui"

CORE_FORMS = {
    "TaskPanel_OrthoArray.ui": 39,
    "TaskPanel_PolarArray.ui": 15,
    "TaskPanel_CircularArray.ui": 19,
    "TaskShapeString.ui": 15,
    "TaskSelectPlane.ui": 26,
    "TaskPanel_SetStyle.ui": 72,
}

QT_MODULES = ("PySide", "PySide2", "PySide6", "shiboken", "shiboken2", "shiboken6")


class QGridLayout:
    """The class-object shape ``findChild`` accepts, without importing Qt."""


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


def _ferrocad_extension_built() -> bool:
    return any(PYTHON_DIR.glob("ferrocad*.so")) or any(
        PYTHON_DIR.glob("ferrocad*.pyd")
    )


_PROBE = r'''
import importlib.machinery, importlib.util, json, sys

python_dir, library, ui_path = sys.argv[1], sys.argv[2], sys.argv[3]
sys.path.insert(0, python_dir)

loader = importlib.machinery.ExtensionFileLoader("ferrocad_ui_py", library)
spec = importlib.util.spec_from_file_location("ferrocad_ui_py", library, loader=loader)
module = importlib.util.module_from_spec(spec)
sys.modules["ferrocad_ui_py"] = module
loader.exec_module(module)

blocked = {"PySide", "PySide2", "PySide6", "shiboken", "shiboken2", "shiboken6"}
import FreeCADGui
before = sorted(name for name in sys.modules if name.split(".")[0] in blocked)
form = FreeCADGui.PySideUic.loadUi(ui_path)
after = sorted(name for name in sys.modules if name.split(".")[0] in blocked)
print("PROBE " + json.dumps({
    "before": before,
    "after": after,
    "widget": form.button_linear_mode.objectName(),
    "title": form.windowTitle(),
    "root_layout": form.layout().className(),
    "root_items": form.layout().count(),
    "linear_items": form.group_linearmode.layout().count(),
}))
'''


@unittest.skipUnless(
    UI_DIR.is_dir(), f"upstream Draft .ui files are not available at {UI_DIR}"
)
@unittest.skipUnless(
    UI_EXTENSION is not None,
    "ferrocad_ui_py is not built; run "
    "`PYO3_USE_ABI3_FORWARD_COMPATIBILITY=1 cargo build -p ferrocad_ui_py "
    "--offline` (the cdylib is target/debug/libferrocad_ui_py.so)",
)
@unittest.skipUnless(
    _ferrocad_extension_built(),
    f"the ferrocad extension is not built in {PYTHON_DIR}",
)
class UiLoaderTests(unittest.TestCase):
    """The six core forms, the property surface, signals and error isolation."""

    @classmethod
    def setUpClass(cls):
        import FreeCADGui

        cls.gui = FreeCADGui

    def load(self, name: str):
        return self.gui.PySideUic.loadUi(str(UI_DIR / name))

    # -- loading and lookup ------------------------------------------------

    def test_the_six_core_forms_load_with_their_widget_counts(self):
        for name, count in CORE_FORMS.items():
            with self.subTest(form=name):
                form = self.load(name)
                self.assertEqual(len(form), count, name)
                self.assertTrue(form.objectName(), name)

        form = self.load("TaskPanel_OrthoArray.ui")
        for widget_name in (
            "DraftOrthoArrayTaskPanel",
            "button_linear_mode",
            "spinbox_n_X",
            "input_X_x",
            "checkbox_fuse",
        ):
            self.assertEqual(getattr(form, widget_name).objectName(), widget_name)

    def test_loadui_accepts_a_qfile_like_object(self):
        class FileLike:
            """The `readAll()`/`fileName()` slice the Rust loader uses."""

            def __init__(self, path):
                self._data = Path(path).read_bytes()
                self._name = str(path)

            def readAll(self):
                return self._data

            def fileName(self):
                return self._name

        form = self.gui.PySideUic.loadUi(
            FileLike(UI_DIR / "TaskPanel_OrthoArray.ui")
        )
        self.assertEqual(len(form), 39)
        self.assertEqual(
            form.button_linear_mode.objectName(), "button_linear_mode"
        )

    # -- typed properties --------------------------------------------------

    def test_text_and_value_round_trip(self):
        form = self.load("TaskPanel_OrthoArray.ui")

        spin = form.spinbox_n_X
        self.assertIsInstance(spin.value(), int)
        spin.setValue(7)
        self.assertEqual(spin.value(), 7)

        field = form.input_X_x
        field.setText("2.5 mm")
        self.assertEqual(field.text(), "2.5 mm")

        label = form.label_n_X
        label.setText("N")
        self.assertEqual(label.text(), "N")

        checkbox = form.checkbox_fuse
        self.assertFalse(checkbox.isChecked())
        checkbox.setChecked(True)
        self.assertTrue(checkbox.isChecked())

        group = form.group_X
        group.setTitle("X axis")
        self.assertEqual(group.title(), "X axis")

        style = self.load("TaskPanel_SetStyle.ui")
        double = style.ScaleMultiplier
        self.assertIsInstance(double.value(), float)
        double.setValue(3.5)
        self.assertAlmostEqual(double.value(), 3.5)

        line = self.load("TaskShapeString.ui").leText
        line.setText("FerroCAD")
        self.assertEqual(line.text(), "FerroCAD")

    def test_combo_box_index_and_text_round_trip(self):
        style = self.load("TaskPanel_SetStyle.ui")
        combo = style.DrawStyle
        self.assertEqual(combo.currentIndex(), 0)
        self.assertEqual(combo.currentText(), "Solid")
        combo.setCurrentIndex(2)
        self.assertEqual(combo.currentIndex(), 2)
        self.assertEqual(combo.currentText(), "Dotted")
        combo.setCurrentIndex(-1)
        self.assertEqual(combo.currentIndex(), -1)
        self.assertEqual(combo.currentText(), "")

    def test_enabled_and_visible_flags(self):
        label = self.load("TaskSelectPlane.ui").label
        self.assertTrue(label.isEnabled())
        self.assertTrue(label.isVisible())
        label.setEnabled(False)
        label.setVisible(False)
        self.assertFalse(label.isEnabled())
        self.assertFalse(label.isVisible())

    # -- signals -----------------------------------------------------------

    def test_signals_connect_emit_and_disconnect(self):
        form = self.load("TaskPanel_OrthoArray.ui")
        seen = []
        button = form.button_linear_mode
        button.clicked.connect(lambda: seen.append("clicked"))
        button.clicked.emit()
        self.assertEqual(seen, ["clicked"])

        self.assertTrue(button.clicked.disconnect())
        button.clicked.emit()
        self.assertEqual(seen, ["clicked"])

    def test_setters_emit_the_matching_signal(self):
        form = self.load("TaskPanel_OrthoArray.ui")

        values = []
        spin = form.spinbox_n_X
        spin.valueChanged.connect(values.append)
        spin.setValue(4)
        spin.valueChanged.emit(9)
        self.assertEqual(values, [4, 9])

        states = []
        form.checkbox_fuse.stateChanged.connect(states.append)
        form.checkbox_fuse.setChecked(True)
        self.assertEqual(states, [True])

        texts = []
        form.label_n_X.textChanged.connect(texts.append)
        form.label_n_X.setText("label")
        self.assertEqual(texts, ["label"])

    # -- findChild and the tree -------------------------------------------

    def test_find_child_by_name_class_and_class_name_string(self):
        form = self.load("TaskPanel_OrthoArray.ui")

        self.assertEqual(form.findChild("spinbox_n_X").objectName(), "spinbox_n_X")
        self.assertEqual(form.findChild("QSpinBox").objectName(), "spinbox_n_X")
        self.assertIsNone(form.findChild("not_a_widget"))

        group = form.group_X
        self.assertEqual(group.findChild("input_X_x").objectName(), "input_X_x")

        class QSpinBox:
            pass

        self.assertEqual(form.findChild(QSpinBox).className(), "QSpinBox")
        self.assertTrue(form.children())
        self.assertTrue(group.children())

    # -- loud failures -----------------------------------------------------

    def test_unknown_members_raise_attribute_error(self):
        form = self.load("TaskPanel_OrthoArray.ui")

        with self.assertRaises(AttributeError):
            form.no_such_widget

        with self.assertRaises(AttributeError) as raised:
            form.spinbox_n_X.text()
        message = str(raised.exception)
        self.assertIn("QSpinBox", message)
        self.assertIn("spinbox_n_X", message)
        self.assertIn("text", message)

        with self.assertRaises(AttributeError):
            form.label_n_X.setValue(1)

        style = self.load("TaskPanel_SetStyle.ui")
        with self.assertRaises(IndexError):
            style.DrawStyle.setCurrentIndex(99)

    def test_accept_and_reject_exist_only_on_dialog_roots(self):
        dialog = self.gui.PySideUic.loadUi(str(UI_DIR / "dialogLayers.ui"))
        self.assertIsNone(dialog.accept())
        self.assertIsNone(dialog.reject())

        widget_form = self.load("TaskPanel_OrthoArray.ui")
        with self.assertRaises(AttributeError):
            widget_form.accept()
        with self.assertRaises(AttributeError):
            widget_form.exec_()
        with self.assertRaises(AttributeError):
            widget_form.accepted

    # -- Q2b: layout surgery ----------------------------------------------

    def test_layout_model_adds_takes_and_reparents(self):
        form = self.load("TaskPanel_OrthoArray.ui")
        root = form.layout()
        self.assertIsNotNone(root)
        self.assertEqual(root.className(), "QGridLayout")
        self.assertEqual(root.objectName(), "gridLayout_3")
        self.assertEqual(root.count(), 9)

        linear = form.group_linearmode.layout()
        self.assertEqual(linear.count(), 0)
        self.assertEqual(linear.className(), "QGridLayout")

        spin = form.spinbox_n_X
        label = form.label_n_X
        spin.setParent(form.group_linearmode)
        label.setParent(form.group_linearmode)
        linear.addWidget(spin, 0, 1)
        linear.addWidget(label, 0, 0, 1, 1)
        self.assertEqual(linear.count(), 2)

        children = [widget.objectName() for widget in form.group_linearmode.children()]
        self.assertIn("spinbox_n_X", children)
        self.assertIn("label_n_X", children)

        # addWidget moved both widgets out of their original grid.
        grid_number = form.findChild(QGridLayout, "grid_number")
        self.assertEqual(grid_number.className(), "QGridLayout")
        self.assertEqual(grid_number.count(), 4)

        taken = linear.takeAt(0)
        self.assertEqual(taken.widget().objectName(), "spinbox_n_X")
        self.assertEqual(linear.count(), 1)
        taken.widget().setParent(None)
        children = [widget.objectName() for widget in form.group_linearmode.children()]
        self.assertNotIn("spinbox_n_X", children)

        with self.assertRaises(IndexError):
            linear.takeAt(99)

    def test_layouts_can_be_moved_and_found(self):
        form = self.load("TaskPanel_OrthoArray.ui")
        group_x = form.group_X.layout()
        self.assertEqual(group_x.count(), 2)

        nested = group_x.takeAt(0)
        self.assertEqual(nested.layout().objectName(), "grid_X")
        self.assertEqual(group_x.count(), 1)

        linear = form.group_linearmode.layout()
        linear.addLayout(nested.layout(), 0, 0)
        self.assertEqual(linear.count(), 1)
        self.assertEqual(linear.itemAt(0).layout().objectName(), "grid_X")
        self.assertIsNone(linear.itemAt(0).widget())
        self.assertIsNone(linear.itemAt(9))

        # A layout is found by name (class object is advisory) or by class.
        self.assertEqual(form.findChild(QGridLayout, "grid_X").className(), "QGridLayout")
        self.assertEqual(form.findChild("grid_X").objectName(), "grid_X")

        with self.assertRaises(AttributeError) as raised:
            linear.setSpacing(4)
        message = str(raised.exception)
        self.assertIn("QGridLayout", message)
        self.assertIn("setSpacing", message)

    # -- Q2b: combo mutation ----------------------------------------------

    def test_combo_mutation_keeps_index_and_text_consistent(self):
        combo = self.load("TaskPanel_SetStyle.ui").DrawStyle
        self.assertEqual(combo.count(), 4)
        self.assertEqual([combo.itemText(i) for i in range(combo.count())],
                         ["Solid", "Dashed", "Dotted", "DashDot"])

        combo.addItem("Dashdot")
        self.assertEqual(combo.count(), 5)
        self.assertEqual(combo.currentIndex(), 0)
        self.assertEqual(combo.currentText(), "Solid")

        combo.setCurrentText("Dashdot")
        self.assertEqual(combo.currentIndex(), 4)
        self.assertEqual(combo.currentText(), "Dashdot")

        combo.addItems(["A", "B"])
        self.assertEqual(combo.count(), 7)
        combo.setCurrentText("B")
        self.assertEqual(combo.currentIndex(), 6)

        # Non-editable boxes ignore an unknown text; the index is unchanged.
        combo.setCurrentText("missing")
        self.assertEqual(combo.currentIndex(), 6)
        self.assertEqual(combo.currentText(), "B")

        combo.clear()
        self.assertEqual(combo.count(), 0)
        self.assertEqual(combo.currentIndex(), -1)
        self.assertEqual(combo.currentText(), "")

        combo.addItem("Fresh")
        self.assertEqual(combo.currentIndex(), 0)
        self.assertEqual(combo.currentText(), "Fresh")

        with self.assertRaises(IndexError):
            combo.itemText(3)

    # -- Q2b: dynamic properties ------------------------------------------

    def test_dynamic_property_round_trip(self):
        form = self.load("TaskPanel_OrthoArray.ui")

        field = form.input_X_x
        self.assertEqual(field.property("rawValue"), 100.0)
        field.setProperty("rawValue", 2.5)
        self.assertEqual(field.property("rawValue"), 2.5)

        spin = form.spinbox_n_X
        self.assertEqual(spin.property("minimum"), 1)
        self.assertEqual(spin.property("maximum"), 1000000)
        spin.setProperty("minimum", 5)
        self.assertEqual(spin.property("minimum"), 5)
        self.assertEqual(spin.value(), 5)

        color = self.load("TaskPanel_SetStyle.ui").ShapeColor
        self.assertEqual(color.property("color"), (0, 0, 0, 255))
        color.setProperty("color", (255, 0, 0, 128))
        self.assertEqual(color.property("color"), (255, 0, 0, 128))
        color.setProperty("color", "#102030")
        self.assertEqual(color.property("color"), (16, 32, 48, 255))

        with self.assertRaises(AttributeError) as raised:
            color.property("fileName")
        message = str(raised.exception)
        self.assertIn("Gui::ColorButton", message)
        self.assertIn("ShapeColor", message)
        self.assertIn("fileName", message)

        with self.assertRaises(AttributeError):
            spin.setProperty("rawValue", 1)
        with self.assertRaises(AttributeError):
            spin.property("bogus")

    def test_spin_box_minimum_and_maximum_clamp(self):
        spin = self.load("TaskPanel_OrthoArray.ui").spinbox_n_X
        self.assertEqual(spin.value(), 2)

        spin.setValue(0)
        self.assertEqual(spin.value(), 1)
        spin.setValue(2000000)
        self.assertEqual(spin.value(), 1000000)

        spin.setValue(7)
        spin.setProperty("maximum", 5)
        self.assertEqual(spin.value(), 5)
        spin.setProperty("minimum", 10)
        self.assertEqual(spin.property("minimum"), 10)
        self.assertEqual(spin.property("maximum"), 10)
        self.assertEqual(spin.value(), 10)

        double = self.load("TaskPanel_SetStyle.ui").ScaleMultiplier
        self.assertEqual(double.property("minimum"), 0.0)
        self.assertEqual(double.property("maximum"), 10000.0)
        double.setValue(-3.5)
        self.assertEqual(double.value(), 0.0)

    def test_file_chooser_file_name(self):
        chooser = self.load("TaskShapeString.ui").fcFontFile
        self.assertEqual(chooser.fileName(), "")
        self.assertEqual(chooser.property("fileName"), "")

        seen = []
        chooser.fileNameChanged.connect(seen.append)
        chooser.setFileName("/tmp/font.ttf")
        self.assertEqual(chooser.fileName(), "/tmp/font.ttf")
        self.assertEqual(chooser.property("fileName"), "/tmp/font.ttf")
        chooser.setProperty("fileName", "/tmp/other.ttf")
        self.assertEqual(chooser.fileName(), "/tmp/other.ttf")
        self.assertEqual(seen, ["/tmp/font.ttf", "/tmp/other.ttf"])

        with self.assertRaises(AttributeError):
            self.load("TaskShapeString.ui").leText.fileName()

    # -- Q2b: dialog semantics --------------------------------------------

    def test_dialog_exec_result_and_signals(self):
        dialog = self.gui.PySideUic.loadUi(str(UI_DIR / "dialogLayers.ui"))
        self.assertEqual(dialog.result(), 0)
        self.assertEqual(dialog.exec_(), 0)

        seen = []
        dialog.accepted.connect(lambda: seen.append("accepted"))
        dialog.rejected.connect(lambda: seen.append("rejected"))

        dialog.accept()
        self.assertEqual(dialog.result(), 1)
        self.assertEqual(dialog.exec_(), 1)
        dialog.reject()
        self.assertEqual(dialog.result(), 0)
        self.assertEqual(dialog.exec_(), 0)
        self.assertEqual(seen, ["accepted", "rejected"])

    # -- the Qt-free property ---------------------------------------------

    def test_loading_a_form_does_not_import_pyside(self):
        library = str(Path(UI_EXTENSION.__file__).resolve())
        process = subprocess.run(
            [
                sys.executable,
                "-c",
                _PROBE,
                str(PYTHON_DIR),
                library,
                str(UI_DIR / "TaskPanel_OrthoArray.ui"),
            ],
            cwd=str(REPO),
            capture_output=True,
            text=True,
        )
        self.assertEqual(
            process.returncode,
            0,
            f"probe failed:\nstdout:\n{process.stdout}\nstderr:\n{process.stderr}",
        )
        payload = json.loads(process.stdout.split("PROBE ", 1)[1])
        self.assertEqual(payload["before"], [])
        self.assertEqual(payload["after"], [])
        self.assertEqual(payload["widget"], "button_linear_mode")
        self.assertTrue(payload["title"])
        self.assertEqual(payload["root_layout"], "QGridLayout")
        self.assertEqual(payload["root_items"], 9)
        self.assertEqual(payload["linear_items"], 0)


if __name__ == "__main__":
    unittest.main()
