"""Tests for the shims that let the upstream Draft workbench import headlessly.

Covers the D0 fixes (``docs/project/record/tasks/d0-draft-import.md``):

* ``lazy_loader.LazyLoader`` imports on first attribute access and caches.
* ``PySide`` is a pure-Python name subset (no Qt runtime), with a loud
  ``AttributeError`` outside its allowlist and a loose-file ``QFile``.
* ``Draft_rc`` is an importable, resource-free stub.
* ``freecad.deprecation`` resolves when upstream's ``src/Ext`` is on the path,
  and is not shadowed by the facade when it is not.

Run::

    PYTHONPATH=python python3 -m unittest discover -s tests
"""

from __future__ import annotations

import importlib
import importlib.machinery
import importlib.util
import os
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
PYTHON_DIR = REPO / "python"
UPSTREAM = Path(os.environ.get("FERROCAD_UPSTREAM", REPO.parent / "freecad-upstream"))
UPSTREAM_EXT = UPSTREAM / "src" / "Ext"
UPSTREAM_DEPRECATION = UPSTREAM_EXT / "freecad" / "deprecation.py"


class _TempModule:
    """Write a throwaway importable module, and clean it up afterwards."""

    def __init__(self, testcase, name):
        self._testcase = testcase
        self._tmp = tempfile.TemporaryDirectory()
        testcase.addCleanup(self._tmp.cleanup)
        testcase.addCleanup(sys.path.remove, self._tmp.name)
        sys.path.insert(0, self._tmp.name)
        self.name = name
        self.path = Path(self._tmp.name) / f"{name}.py"
        testcase.addCleanup(sys.modules.pop, name, None)
        importlib.invalidate_caches()

    def write(self, source: str) -> Path:
        self.path.write_text(source)
        importlib.invalidate_caches()
        return self.path


class LazyLoaderTests(unittest.TestCase):
    def test_not_imported_until_the_attribute_is_read(self):
        import lazy_loader.lazy_loader as lz

        probe = _TempModule(self, "d0_lazy_late")
        marker = Path(probe.path).with_suffix(".imported")
        probe.write(
            "from pathlib import Path\n"
            f"Path({str(marker)!r}).write_text('imported')\n"
            "VALUE = 42\n"
        )

        namespace = {}
        loader = lz.LazyLoader("Probe", namespace, probe.name)

        self.assertFalse(marker.exists(), "the module was imported too early")
        self.assertNotIn(probe.name, sys.modules)

        self.assertEqual(loader.VALUE, 42)
        self.assertTrue(marker.exists())

    def test_first_read_replaces_the_global_and_caches(self):
        import lazy_loader.lazy_loader as lz

        probe = _TempModule(self, "d0_lazy_cached")
        counter = Path(probe.path).with_suffix(".count")
        probe.write(
            "from pathlib import Path\n"
            f"_path = Path({str(counter)!r})\n"
            "_path.write_text(_path.read_text() + 'x' if _path.exists() else 'x')\n"
            "VALUE = object()\n"
        )
        namespace = {}
        # Bind the loader as the module would: `Probe = lz.LazyLoader(...)`.
        namespace["Probe"] = lz.LazyLoader("Probe", namespace, probe.name)
        loader = namespace["Probe"]

        first = loader.VALUE
        imported = sys.modules[probe.name]
        self.assertIs(namespace["Probe"], imported, "the global was not replaced")

        # Drop the import-system cache: the loader's own cache must still answer,
        # and the module body must not run a second time.
        del sys.modules[probe.name]
        second = loader.VALUE

        self.assertIs(first, second)
        self.assertEqual(counter.read_text(), "x")

    def test_import_error_is_raised_on_access_not_construction(self):
        import lazy_loader.lazy_loader as lz

        probe = _TempModule(self, "d0_lazy_broken")
        probe.write("raise RuntimeError('module body ran')\n")

        loader = lz.LazyLoader("Probe", {}, probe.name)  # no import yet
        with self.assertRaises(RuntimeError):
            loader.anything

    def test_package_reexports_the_loader(self):
        import lazy_loader

        self.assertIs(
            lazy_loader.LazyLoader,
            importlib.import_module("lazy_loader.lazy_loader").LazyLoader,
        )


class PySideStubTests(unittest.TestCase):
    """Q0a/Q0b: the shim is a subset, not an alias, and reads mod resources."""

    def setUp(self):
        import PySide._qt_stub as stub

        stub.clear_resource_cache()
        self.addCleanup(stub.clear_resource_cache)

    def test_modules_are_pure_python_stub_modules(self):
        import PySide
        import PySide._qt_stub as stub
        from PySide import QtCore, QtGui, QtWidgets

        self.assertTrue(PySide.USING_STUB)
        self.assertIs(getattr(PySide, stub.STUB_MARKER), True)
        for name, module in (
            ("QtCore", QtCore),
            ("QtGui", QtGui),
            ("QtWidgets", QtWidgets),
        ):
            self.assertIsInstance(module, stub.QtStubModule)
            self.assertEqual(module.__name__, f"PySide.{name}")
            self.assertIs(getattr(module, stub.STUB_MARKER), True)
            self.assertIs(sys.modules[f"PySide.{name}"], module)

        # The old alias shared the module object with PySide6; the stub must not.
        for loaded_name, loaded in list(sys.modules.items()):
            if loaded_name.startswith(("PySide6", "shiboken")):
                self.assertIsNot(loaded, QtCore)
                self.assertIsNot(loaded, QtGui)
                self.assertIsNot(loaded, QtWidgets)

    def test_translate_noop_is_available_at_module_and_package_level(self):
        import PySide
        from PySide import QtCore, QtWidgets
        from PySide.QtCore import QT_TRANSLATE_NOOP

        self.assertEqual(QT_TRANSLATE_NOOP("draft", "Rectangle"), "Rectangle")
        self.assertEqual(PySide.QT_TRANSLATE_NOOP("draft", "Rectangle"), "Rectangle")
        self.assertEqual(QtCore.QCoreApplication.translate("draft", "Line", None), "Line")
        self.assertEqual(QtWidgets.QApplication.translate("draft", "Line"), "Line")

    def test_qt6_has_no_unicode_utf8_and_the_shim_matches(self):
        # ``draftutils/translate.py`` probes this in a try/except AttributeError.
        from PySide import QtWidgets

        with self.assertRaises(AttributeError):
            QtWidgets.QApplication.UnicodeUTF8

    def test_unknown_names_raise_with_the_module_and_name(self):
        import PySide
        from PySide import QtCore, QtGui

        with self.assertRaises(AttributeError) as raised:
            QtCore.QDir
        message = str(raised.exception)
        self.assertIn("PySide.QtCore", message)
        self.assertIn("QDir", message)

        # QtGui intentionally exports no names: nothing is evaluated at import
        # time by the headless Draft path, so everything fails loudly.
        with self.assertRaises(AttributeError) as raised:
            QtGui.QIcon
        self.assertIn("PySide.QtGui", str(raised.exception))
        self.assertIn("QIcon", str(raised.exception))

        with self.assertRaises(AttributeError) as raised:
            PySide.QtSvg
        self.assertIn("PySide", str(raised.exception))
        self.assertIn("QtSvg", str(raised.exception))

    def test_qt_svg_is_not_provided(self):
        # QtSvg is not a default requirement (qt-strategy.md §4).
        self.assertNotIn("PySide.QtSvg", sys.modules)
        with self.assertRaises(ModuleNotFoundError):
            importlib.import_module("PySide.QtSvg")

    def test_io_device_flags_match_qt(self):
        from PySide import QtCore

        self.assertEqual(QtCore.QIODevice.ReadOnly, 1)
        self.assertEqual(QtCore.QIODevice.WriteOnly, 2)
        self.assertEqual(QtCore.QFile.Text, 16)
        self.assertEqual(QtCore.QIODevice.ReadOnly | QtCore.QFile.Text, 17)

    def test_signal_and_legacy_connect(self):
        from PySide import QtCore

        class Emitter(QtCore.QObject):
            def __init__(self):
                super().__init__()
                self.valueChanged = self.signal("valueChanged")

        emitter = Emitter()
        seen = []
        QtCore.QObject.connect(
            emitter, QtCore.SIGNAL("valueChanged(double)"), seen.append
        )
        emitter.valueChanged.emit(1.5)
        self.assertEqual(seen, [1.5])

        self.assertEqual(QtCore.SIGNAL("clicked()").name, "clicked")
        with self.assertRaises(AttributeError) as raised:
            QtCore.QObject.connect(emitter, QtCore.SIGNAL("missing()"), seen.append)
        self.assertIn("missing", str(raised.exception))

    def test_timer_single_shot_zero_runs_immediately(self):
        from PySide import QtCore

        seen = []
        QtCore.QTimer.singleShot(0, lambda: seen.append("fired"))
        self.assertEqual(seen, ["fired"])

    def test_locale_decimal_point_and_font_paths(self):
        from PySide import QtCore

        self.assertEqual(QtCore.QLocale().decimalPoint(), ".")
        paths = QtCore.QStandardPaths.standardLocations(
            QtCore.QStandardPaths.FontsLocation
        )
        self.assertIsInstance(paths, list)
        for path in paths:
            self.assertTrue(os.path.isdir(path), path)

    def test_resource_paths_read_a_mods_loose_files(self):
        from PySide import QtCore

        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp) / "DemoMod" / "Resources" / "ui"
            folder.mkdir(parents=True)
            (folder / "preferences-demo.ui").write_text("<ui>demo</ui>")
            sys.path.insert(0, tmp)
            try:
                handle = QtCore.QFile(":/ui/preferences-demo.ui")
                self.assertTrue(handle.exists())
                self.assertTrue(
                    handle.open(QtCore.QIODevice.ReadOnly | QtCore.QFile.Text)
                )
                self.assertEqual(handle.readAll().data().decode(), "<ui>demo</ui>")
                handle.close()
                self.assertFalse(handle.isOpen())
            finally:
                sys.path.remove(tmp)

    def test_read_is_incremental(self):
        from PySide import QtCore

        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "Resources" / "ui"
            root.mkdir(parents=True)
            (root / "tiny.ui").write_text("abcdef")
            sys.path.insert(0, tmp)
            try:
                handle = QtCore.QFile(":/ui/tiny.ui")
                self.assertTrue(handle.open(QtCore.QIODevice.ReadOnly))
                self.assertEqual(bytes(handle.read(2)), b"ab")
                self.assertEqual(handle.readAll().data(), b"cdef")
                self.assertTrue(handle.atEnd())
                handle.close()
            finally:
                sys.path.remove(tmp)

    def test_a_missing_resource_returns_false_like_qt(self):
        from PySide import QtCore

        handle = QtCore.QFile(":/ui/does-not-exist.ui")
        self.assertFalse(handle.exists())
        self.assertFalse(handle.open(QtCore.QIODevice.ReadOnly))

    def test_plain_paths_still_read_from_disk(self):
        from PySide import QtCore

        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "plain.txt"
            path.write_text("hello")
            handle = QtCore.QFile(str(path))
            self.assertTrue(handle.open(QtCore.QIODevice.ReadOnly | QtCore.QFile.Text))
            self.assertEqual(handle.readAll().data().decode(), "hello")
            handle.close()

    def test_writing_through_qfile_is_loud(self):
        from PySide import QtCore

        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "out.txt"
            handle = QtCore.QFile(str(path))
            with self.assertRaises(NotImplementedError) as raised:
                handle.open(QtCore.QIODevice.WriteOnly)
            self.assertIn("read-only", str(raised.exception))


class DraftResourceStubTests(unittest.TestCase):
    def test_import_has_a_truthy_name_and_noop_init(self):
        import Draft_rc

        self.assertTrue(Draft_rc.__name__)
        self.assertIsNone(Draft_rc.qInitResources())
        self.assertIsNone(Draft_rc.qCleanupResources())


class FreecadExtPackageTests(unittest.TestCase):
    def setUp(self):
        self._clear_modules()
        self.addCleanup(self._clear_modules)

    @staticmethod
    def _clear_modules():
        for name in ("freecad.deprecation", "freecad"):
            sys.modules.pop(name, None)

    @unittest.skipUnless(
        UPSTREAM_DEPRECATION.is_file(),
        f"upstream src/Ext/freecad is not available at {UPSTREAM}",
    )
    def test_deprecated_imports_with_the_upstream_tree_on_the_path(self):
        sys.path.insert(0, str(UPSTREAM_EXT))
        self.addCleanup(sys.path.remove, str(UPSTREAM_EXT))

        from freecad.deprecation import deprecated

        self.assertTrue(callable(deprecated))

    def test_facade_does_not_shadow_the_upstream_package(self):
        # `python/FreeCAD` is the facade; a `python/freecad` directory would
        # shadow upstream's package on case-insensitive filesystems, and would
        # make the "not installed" error misleading.
        self.assertFalse((PYTHON_DIR / "freecad").exists())
        spec = importlib.machinery.PathFinder.find_spec("freecad", [str(PYTHON_DIR)])
        self.assertIsNone(spec)

    def test_import_without_the_tree_fails_with_a_clear_error(self):
        if importlib.util.find_spec("freecad") is not None:
            self.skipTest("a freecad package is installed outside the upstream checkout")

        with self.assertRaises(ModuleNotFoundError) as raised:
            importlib.import_module("freecad.deprecation")
        self.assertIn("freecad", str(raised.exception))

    @unittest.skipUnless(
        UPSTREAM_DEPRECATION.is_file(),
        f"upstream src/Ext/freecad is not available at {UPSTREAM}",
    )
    def test_typing_extensions_is_available(self):
        # `freecad.deprecation` — upstream's module, not FerroCAD — imports
        # `typing_extensions.deprecated`. FerroCAD itself does not depend on it, so
        # a minimal CI interpreter without the upstream tree has nothing to check.
        module = importlib.import_module("typing_extensions")
        self.assertTrue(callable(module.deprecated))


if __name__ == "__main__":
    unittest.main()
