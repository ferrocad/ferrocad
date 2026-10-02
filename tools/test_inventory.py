"""Tests for ``tools/inventory.py`` (M3a surface inventory).

Hermetic: builds a tiny ``.pyi`` tree in a temp dir and asserts the parser and
aggregation, so it runs without the upstream checkout. A separate integration
test (``test_discover_upstream``) runs only when ``../../freecad-upstream`` is
present.

Run::

    PYTHONPATH=tools python3 tools/test_inventory.py
"""

import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, os.path.dirname(__file__))

from inventory import classify_area, discover, parse_stub, summarize  # noqa: E402

FIXTURE_CLASS = '''\
from __future__ import annotations
from typing import Final, overload


class Doc:
    Name: Final[str] = ""
    Count: int = 0

    @overload
    def get(self) -> str: ...
    @overload
    def get(self, idx: int, /) -> str: ...
    def get(self, idx: int | None = None) -> str: ...

    def add(self, a: int, b: int = 1, *, c: str = "x") -> None: ...

    @classmethod
    def make(cls, name: str) -> "Doc": ...
'''

FIXTURE_MODULE = '''\
from __future__ import annotations


def version() -> list[str]: ...
def new(name: str, /) -> "Doc": ...
'''


class TestParseStub(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)

    def _write(self, rel, text):
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
        return path

    def test_parse_class_stub(self):
        path = self._write("src/App/Doc.pyi", FIXTURE_CLASS)
        mod = parse_stub(path, "App")

        self.assertEqual(mod.area, "App")
        self.assertEqual(len(mod.classes), 1)

        doc = mod.classes[0]
        self.assertEqual(doc.name, "Doc")
        self.assertEqual(doc.bases, [])

        # attributes
        names = {a.name: a for a in doc.attributes}
        self.assertEqual(names["Name"].annotation, "Final[str]")
        self.assertEqual(names["Count"].annotation, "int")

        # overloaded method merged into one Method with three signatures
        get = next(m for m in doc.methods if m.name == "get")
        self.assertTrue(get.is_overloaded)
        self.assertEqual(len(get.signatures), 3)

        # positional-only, default, keyword-only, and return type
        add = next(m for m in doc.methods if m.name == "add")
        self.assertEqual(add.signatures[0].returns, "None")
        self.assertEqual([(p.name, p.kind, p.default) for p in add.signatures[0].params],
                         [("a", "pos", False), ("b", "pos", True), ("c", "kwonly", True)])

        # classmethod flag
        make = next(m for m in doc.methods if m.name == "make")
        self.assertTrue(make.is_classmethod)

    def test_parse_module_stub(self):
        path = self._write("src/App/FreeCAD.module.pyi", FIXTURE_MODULE)
        mod = parse_stub(path, "App")

        self.assertEqual([f.name for f in mod.functions], ["version", "new"])
        version = mod.functions[0]
        self.assertEqual(version.signatures[0].returns, "list[str]")
        new = mod.functions[1]
        self.assertEqual([(p.name, p.kind) for p in new.signatures[0].params],
                         [("name", "posonly")])


class TestDiscoveryAndSummary(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        for rel, text in [
            ("src/App/Doc.pyi", FIXTURE_CLASS),
            ("src/App/FreeCAD.module.pyi", FIXTURE_MODULE),
            ("src/Mod/Part/Box.pyi", "class Box:\n    def volume(self) -> float: ...\n"),
        ]:
            path = self.root / rel
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text, encoding="utf-8")

    def test_discover_and_summarize(self):
        modules = discover(self.root)
        summary = summarize(modules)

        self.assertEqual(summary["totals"]["files"], 3)
        self.assertEqual(summary["totals"]["classes"], 2)   # Doc + Box
        self.assertEqual(summary["totals"]["functions"], 2)  # version + new

        areas = summary["areas"]
        self.assertIn("App", areas)
        self.assertIn("Mod.Part", areas)
        self.assertEqual(areas["App"]["files"], 2)
        self.assertEqual(areas["Mod.Part"]["classes"], 1)


class TestLegacyXmlGuard(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        (self.root / "src" / "App").mkdir(parents=True)
        (self.root / "src" / "App" / "Doc.pyi").write_text(FIXTURE_CLASS, encoding="utf-8")

    def _discover_stderr(self):
        import contextlib
        import io

        buf = io.StringIO()
        with contextlib.redirect_stderr(buf):
            modules = discover(self.root)
        return modules, buf.getvalue()

    def test_no_warning_on_migrated_tree(self):
        _, err = self._discover_stderr()
        self.assertNotIn("legacy", err)

    def test_warns_on_pre_migration_xml(self):
        # A legacy binding with no .pyi sibling would be invisible to a .pyi walk.
        (self.root / "src" / "App" / "LegacyPy.xml").write_text("<xml/>", encoding="utf-8")
        _, err = self._discover_stderr()
        self.assertIn("legacy", err)
        self.assertIn("XML->pyi migration", err)


class TestClassifyArea(unittest.TestCase):
    def test_areas(self):
        self.assertEqual(classify_area(Path("App/Document.pyi")), "App")
        self.assertEqual(classify_area(Path("Base/Vector.pyi")), "Base")
        self.assertEqual(classify_area(Path("Gui/Application.pyi")), "Gui")
        self.assertEqual(classify_area(Path("Mod/Part/Box.pyi")), "Mod.Part")
        self.assertEqual(
            classify_area(Path("Tools/typing/inputs/overlays/PySide/QtCore.pyi")),
            "PySide",
        )


UPSTREAM = Path(__file__).resolve().parents[2] / "freecad-upstream"


@unittest.skipUnless((UPSTREAM / "src").is_dir(), "upstream checkout not present")
class TestUpstreamIntegration(unittest.TestCase):
    def test_discover_upstream(self):
        root = UPSTREAM
        modules = discover(root)
        summary = summarize(modules)
        # Regression guard: the surface should be substantial and parse cleanly.
        self.assertGreaterEqual(summary["totals"]["files"], 300)
        self.assertGreaterEqual(summary["totals"]["classes"], 300)
        self.assertGreaterEqual(summary["totals"]["methods"], 2000)


if __name__ == "__main__":
    unittest.main()
