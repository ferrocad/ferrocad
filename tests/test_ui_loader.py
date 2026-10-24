"""Q2a: ``FreeCADGui.PySideUic.loadUi`` and the headless form proxies.

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


if __name__ == "__main__":
    unittest.main()
