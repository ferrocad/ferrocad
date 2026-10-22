"""Tests for the shims that let the upstream Draft workbench import headlessly.

Covers the D0 fixes (``docs/tasks/d0-draft-import.md``):

* ``lazy_loader.LazyLoader`` imports on first attribute access and caches.
* ``PySide`` aliases the installed PySide6 and tolerates absent modules.
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


class PySideAliasTests(unittest.TestCase):
    def test_qtcore_and_qtwidgets_are_pyside6_modules(self):
        from PySide import QtCore, QtWidgets

        import PySide6.QtCore
        import PySide6.QtWidgets

        self.assertIs(QtCore, PySide6.QtCore)
        self.assertIs(QtWidgets, PySide6.QtWidgets)
        self.assertIs(sys.modules["PySide.QtCore"], sys.modules["PySide6.QtCore"])

    def test_translate_noop_is_available_at_module_and_package_level(self):
        import PySide
        from PySide.QtCore import QT_TRANSLATE_NOOP

        self.assertEqual(QT_TRANSLATE_NOOP("draft", "Rectangle"), "Rectangle")
        self.assertEqual(PySide.QT_TRANSLATE_NOOP("draft", "Rectangle"), "Rectangle")

    def test_pyside6_is_not_imported_twice(self):
        import PySide.QtGui

        module = importlib.import_module("PySide6.QtGui")
        self.assertIs(PySide.QtGui, module)
        # The alias is the same object, not a copy: one entry holds it.
        holding = [k for k, v in sys.modules.items() if v is module]
        self.assertIn("PySide.QtGui", holding)
        self.assertIn("PySide6.QtGui", holding)

    def test_resource_paths_read_a_mods_loose_files(self):
        from PySide import QtCore

        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp) / "DemoMod" / "Resources" / "ui"
            folder.mkdir(parents=True)
            (folder / "preferences-demo.ui").write_text("<ui>demo</ui>")
            sys.path.insert(0, tmp)
            try:
                handle = QtCore.QFile(":/ui/preferences-demo.ui")
                self.assertTrue(
                    handle.open(QtCore.QIODevice.ReadOnly | QtCore.QFile.Text)
                )
                self.assertEqual(handle.readAll().data().decode(), "<ui>demo</ui>")
                handle.close()
            finally:
                sys.path.remove(tmp)

    def test_plain_paths_still_go_to_qt(self):
        from PySide import QtCore

        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "plain.txt"
            path.write_text("hello")
            handle = QtCore.QFile(str(path))
            self.assertTrue(
                handle.open(QtCore.QIODevice.ReadOnly | QtCore.QFile.Text)
            )
            self.assertEqual(handle.readAll().data().decode(), "hello")
            handle.close()

    def test_absent_qtsvg_is_tolerated(self):
        try:
            importlib.import_module("PySide6.QtSvg")
        except ImportError:
            have_svg = False
        else:
            have_svg = True

        if have_svg:
            from PySide import QtSvg

            self.assertIs(QtSvg, importlib.import_module("PySide6.QtSvg"))
        else:
            self.assertNotIn("PySide.QtSvg", sys.modules)
            with self.assertRaises(ModuleNotFoundError):
                importlib.import_module("PySide.QtSvg")
        # Either way, the rest of the package still imports.
        from PySide import QtCore

        self.assertTrue(hasattr(QtCore, "QCoreApplication"))


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

    def test_typing_extensions_is_available(self):
        # `freecad.deprecation` imports `typing_extensions.deprecated`.
        module = importlib.import_module("typing_extensions")
        self.assertTrue(callable(module.deprecated))


if __name__ == "__main__":
    unittest.main()
