"""Tests for the generated-documentation tooling.

Hermetic: exercises discovery and rendering with synthetic captures, so it needs
no GPU, no workflows, and no committed pages.

Run::

    PYTHONPATH=python python3 tools/test_docs_tooling.py
"""

import os
import sys
import unittest
from pathlib import Path

TOOLS = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(TOOLS)

sys.path.insert(0, TOOLS)
sys.path.insert(0, os.path.join(TOOLS, "workflows"))
sys.path.insert(0, os.path.join(REPO, "python"))

import _capture  # noqa: E402
import api_docs  # noqa: E402
import userdocs  # noqa: E402


def capture(**overrides):
    base = {
        "slug": "demo",
        "title": "A demo workflow",
        "summary": "One line.",
        "requires": ["the facade"],
        "caveats": [],
        "ops": [
            {"kind": "step", "caption": "First"},
            {"kind": "prose", "text": "Explained."},
            {"kind": "code", "source": "x = 1"},
            {"kind": "table", "headers": ["Read back", "Value"], "rows": [["x", "1"]]},
            {"kind": "json", "label": "snapshot", "text": "{}"},
            {"kind": "step", "caption": "Second"},
            {"kind": "note", "text": "A note."},
        ],
    }
    base.update(overrides)
    return base


class TestDiscovery(unittest.TestCase):
    def test_workflows_are_discovered_in_reading_order(self):
        names = [p.name for p in userdocs.discover()]
        self.assertTrue(names, "expected at least one workflow")
        self.assertEqual(names, sorted(names))
        self.assertTrue(names[0][0].isdigit(), "a numeric prefix orders the pages")
        self.assertNotIn("_capture.py", names)


class TestRenderPage(unittest.TestCase):
    def test_steps_are_numbered_and_ops_keep_their_order(self):
        page = userdocs.render_page(capture(), Path("01_demo.py"))
        self.assertIn("## 1. First", page)
        self.assertIn("## 2. Second", page)
        self.assertLess(page.index("## 1. First"), page.index("## 2. Second"))
        self.assertLess(page.index("x = 1"), page.index("| Read back | Value |"))

    def test_the_source_is_linked_and_the_page_is_marked_generated(self):
        page = userdocs.render_page(capture(), Path("01_demo.py"))
        self.assertIn("../../tools/workflows/01_demo.py", page)
        self.assertIn("Generated page", page)

    def test_table_cells_escape_pipes(self):
        page = userdocs.render_page(
            capture(
                ops=[
                    {
                        "kind": "table",
                        "headers": ["A"],
                        "rows": [["one|two"]],
                    }
                ]
            ),
            Path("01_demo.py"),
        )
        self.assertIn(r"one\|two", page)

    def test_a_missing_image_says_so_instead_of_pointing_at_nothing(self):
        page = userdocs.render_page(
            capture(
                ops=[
                    {
                        "kind": "image",
                        "name": "not-there.png",
                        "alt": "the viewport",
                        "caption": None,
                    }
                ]
            ),
            Path("01_demo.py"),
        )
        self.assertIn("No image captured", page)
        self.assertNotIn("![the viewport]", page)

    def test_rendering_is_deterministic(self):
        self.assertEqual(
            userdocs.render_page(capture(), Path("01_demo.py")),
            userdocs.render_page(capture(), Path("01_demo.py")),
        )


class TestRenderIndex(unittest.TestCase):
    def test_every_workflow_is_listed(self):
        captures = [
            (Path("01_a.py"), capture(slug="a", title="A")),
            (Path("02_b.py"), capture(slug="b", title="B")),
        ]
        index = userdocs.render_index(captures)
        self.assertIn("[A](a.md)", index)
        self.assertIn("[B](b.md)", index)
        self.assertIn("2 workflows", index)


class TestCells(unittest.TestCase):
    def test_scalars_and_containers(self):
        self.assertEqual(_capture._cell(True), "true")
        self.assertEqual(_capture._cell(1.5), "1.5")
        self.assertEqual(_capture._cell(["a"]), '["a"]')
        self.assertEqual(_capture._cell(None), "—")

    def test_an_object_without_json_uses_its_str(self):
        class Point:
            def __str__(self):
                return "Vector (1, 2, 3)"

        self.assertEqual(_capture._cell(Point()), "Vector (1, 2, 3)")


class TestApiDocs(unittest.TestCase):
    def test_page_names_are_the_dotted_module_name(self):
        self.assertEqual(api_docs._page_name("FreeCAD.Base"), "FreeCAD.Base")

    def test_a_module_description_renders_its_sections(self):
        description = {
            "module": "Demo",
            "summary": "A demo module.",
            "functions": [
                {"name": "f", "signature": "(x)", "summary": "Does f."}
            ],
            "classes": [],
            "objects": [],
            "values": [{"name": "v", "summary": "", "value": 3}],
        }
        page = api_docs.render_module(description, "The demo.", None)
        self.assertIn("# `Demo`", page)
        self.assertIn("`f(x)`", page)
        self.assertIn("Does f.", page)
        self.assertIn("`v`", page)


if __name__ == "__main__":
    unittest.main()
