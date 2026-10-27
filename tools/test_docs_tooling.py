"""Tests for the generated-documentation tooling.

Hermetic: exercises discovery and rendering with synthetic captures, so it needs
no GPU, no workflows, and no committed pages.

Run::

    PYTHONPATH=python python3 tools/test_docs_tooling.py
"""

import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

TOOLS = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(TOOLS)

sys.path.insert(0, TOOLS)
sys.path.insert(0, os.path.join(TOOLS, "workflows"))
sys.path.insert(0, os.path.join(REPO, "python"))

import _capture  # noqa: E402
import api_docs  # noqa: E402
import build_book  # noqa: E402
import userdocs  # noqa: E402


def capture(**overrides):
    base = {
        "slug": "demo",
        "quadrant": "how-to",
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

    def test_an_existing_image_links_the_sibling_images_folder(self):
        with tempfile.TemporaryDirectory() as tmp:
            Path(tmp, "demo.png").write_bytes(b"png")
            with mock.patch.object(userdocs, "IMAGE_DIR", Path(tmp)):
                page = userdocs.render_page(
                    capture(
                        ops=[
                            {
                                "kind": "image",
                                "name": "demo.png",
                                "alt": "the viewport",
                                "caption": "A caption.",
                            }
                        ]
                    ),
                    Path("01_demo.py"),
                )
        # The page is in `<quadrant>/`, so the committed image is one level up.
        self.assertIn("![the viewport](../images/demo.png)", page)
        self.assertIn("*A caption.*", page)
        self.assertNotIn("No image captured", page)

    def test_rendering_is_deterministic(self):
        self.assertEqual(
            userdocs.render_page(capture(), Path("01_demo.py")),
            userdocs.render_page(capture(), Path("01_demo.py")),
        )


class TestRenderIndex(unittest.TestCase):
    def test_every_workflow_is_listed(self):
        captures = [
            (Path("01_a.py"), capture(slug="a", title="A", quadrant="tutorials")),
            (Path("02_b.py"), capture(slug="b", title="B", quadrant="how-to")),
        ]
        index = userdocs.render_index(captures)
        self.assertIn("[A](tutorials/a.md)", index)
        self.assertIn("[B](how-to/b.md)", index)
        self.assertIn("2 workflows", index)

    def test_the_sections_follow_diataxis_and_include_reference(self):
        index = userdocs.render_index(
            [
                (Path("00_t.py"), capture(slug="t", quadrant="tutorials")),
                (Path("01_h.py"), capture(slug="h", quadrant="how-to")),
            ]
        )
        order = [
            index.index("## Tutorials"),
            index.index("## How-to guides"),
            index.index("## Reference"),
            index.index("## Explanation"),
        ]
        self.assertEqual(order, sorted(order))
        self.assertIn("../python-api/index.md", index)

    def test_an_authored_explanation_page_is_listed(self):
        index = userdocs.render_index([])
        self.assertIn("how-the-shell-talks-to-the-document.md", index)


class TestImageSpecs(unittest.TestCase):
    """The `image()` op records what the renderer needs, without a GPU."""

    def capture(self):
        return _capture.Capture(slug="demo", quadrant="how-to", title="Demo", summary="")

    def test_a_mesh_op_records_its_geometry_and_display_attributes(self):
        cap = self.capture()
        mesh = {
            "positions": [[0, 0, 0], [1, 0, 0], [0, 1, 0]],
            "triangles": [0, 1, 2],
            "edges": [0, 1],
            "color": [0.1, 0.2, 0.3, 1.0],
            "display": "shaded_edges",
            "object": "Slab",
        }
        cap.image(
            "demo.png",
            "the slab",
            camera={"eye": [3, 3, 3], "target": [0, 0, 0]},
            mesh=mesh,
        )
        op = cap.ops[-1]
        self.assertEqual(op["kind"], "image")
        self.assertEqual(op["name"], "demo.png")
        spec = op["spec"]
        self.assertEqual(spec["camera"]["eye"], [3.0, 3.0, 3.0])
        shape = spec["shapes"][0]
        self.assertEqual(shape["geometry"]["mesh"]["triangles"], [0, 1, 2])
        self.assertNotIn("object", shape["geometry"]["mesh"])
        self.assertEqual(shape["color"], [0.1, 0.2, 0.3, 1.0])
        self.assertEqual(shape["draw_mode"], "shaded_edges")

    def test_a_box_op_records_the_renderer_primitive(self):
        cap = self.capture()
        cap.image(
            "box.png",
            "a box",
            shape={"box": {"size": [105.0, 20.0, 10.0], "origin": [0.0, 0.0, 0.0]}},
            camera={"eye": [1, 2, 3], "target": [0, 0, 0], "fov_y": 0.6},
            size=[960, 600],
        )
        spec = cap.ops[-1]["spec"]
        self.assertEqual(spec["size"], [960, 600])
        self.assertEqual(
            spec["shapes"][0]["geometry"]["box"]["size"], [105.0, 20.0, 10.0]
        )
        self.assertEqual(spec["camera"]["fov_y"], 0.6)

    def test_a_snapshot_needs_a_camera(self):
        with self.assertRaises(ValueError):
            self.capture().image("x.png", "alt", shape={"box": {"size": [1, 1, 1]}})

    def test_image_names_stay_inside_the_images_folder(self):
        for name in ("../escape.png", "a/b.png", "", "not-a-png.gif"):
            with self.assertRaises(RuntimeError, msg=name):
                userdocs._safe_image_name(name)
        self.assertEqual(userdocs._safe_image_name("first-model.png"), "first-model.png")


class TestCapture(unittest.TestCase):
    def test_no_adapter_leaves_the_committed_images_alone(self):
        with tempfile.TemporaryDirectory() as tmp:
            image_dir = Path(tmp, "images")
            image_dir.mkdir()
            existing = image_dir / "demo.png"
            existing.write_bytes(b"old bytes")

            # The example's exit code 3 is "no GPU adapter".
            fake = Path(tmp, "no-gpu.sh")
            fake.write_text("#!/bin/sh\nexit 3\n")
            fake.chmod(0o755)

            cap = capture(
                ops=[
                    {
                        "kind": "image",
                        "name": "demo.png",
                        "alt": "a",
                        "caption": None,
                        "spec": {"camera": {}, "shapes": []},
                    }
                ]
            )
            with mock.patch.object(userdocs, "IMAGE_DIR", image_dir):
                written, unchanged, degraded = userdocs.capture_images(
                    [(Path("01_demo.py"), cap)], fake
                )

            self.assertTrue(degraded)
            self.assertEqual((written, unchanged), (0, 0))
            self.assertEqual(existing.read_bytes(), b"old bytes")


class TestQuadrants(unittest.TestCase):
    def test_a_tutorial_alias_normalises(self):
        cap = _capture.Capture(slug="x", quadrant="tutorial", title="X", summary="")
        self.assertEqual(cap.quadrant, "tutorials")

    def test_an_unknown_quadrant_is_rejected(self):
        with self.assertRaises(ValueError):
            _capture.Capture(slug="x", quadrant="cookbook", title="X", summary="")

    def test_generated_pages_carry_the_marker(self):
        page = userdocs.render_page(capture(), Path("01_demo.py"))
        self.assertTrue(page.startswith(userdocs.GENERATED_MARKER))


class TestBook(unittest.TestCase):
    def test_a_link_that_leaves_the_book_becomes_a_github_url(self):
        source = build_book.USER / "explanation" / "x.md"
        out = build_book._rewrite_links("[x](../../workbenches/conformance.md)", source)
        self.assertIn(
            "https://github.com/ferrocad/ferrocad/blob/main/docs/workbenches/conformance.md",
            out,
        )

    def test_a_link_inside_the_book_stays_relative(self):
        source = build_book.USER / "explanation" / "x.md"
        out = build_book._rewrite_links("[x](../how-to/toy-workbench.md)", source)
        self.assertIn("(../how-to/toy-workbench.md)", out)

    def test_a_directory_link_becomes_its_index_page(self):
        source = build_book.USER / "explanation" / "x.md"
        out = build_book._rewrite_links("[api](../../python-api/)", source)
        # `user/` is dropped on assembly, so one `../` is enough in the book.
        self.assertIn("(../python-api/index.md)", out)

    def test_user_is_dropped_from_the_published_path(self):
        self.assertEqual(
            build_book._book_path(build_book.USER / "tutorials" / "x.md").as_posix(),
            "tutorials/x.md",
        )
        self.assertEqual(
            build_book._book_path(build_book.API / "FreeCAD.md").as_posix(),
            "python-api/FreeCAD.md",
        )

    def test_a_cross_section_link_is_recomputed_for_the_book_layout(self):
        source = build_book.USER / "how-to" / "x.md"
        out = build_book._rewrite_links("[ref](../../python-api/FreeCAD.md)", source)
        self.assertIn("(../python-api/FreeCAD.md)", out)

    def test_site_url_is_only_emitted_when_set(self):
        self.assertNotIn("site-url", build_book.book_toml(""))
        self.assertIn(
            'site-url = "https://ferrocad.github.io/docs/"',
            build_book.book_toml("https://ferrocad.github.io/docs/"),
        )

    def test_the_reference_landing_page_is_index(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            src = build_book.assemble(Path(tmp))
            summary = (src / "SUMMARY.md").read_text()
        self.assertIn("(python-api/index.md)", summary)
        self.assertNotIn("python-api/README.md", summary)

    def test_external_urls_are_untouched(self):
        source = build_book.USER / "README.md"
        text = "[r](https://example.com/x)"
        self.assertEqual(build_book._rewrite_links(text, source), text)

    def test_the_workflow_prefix_orders_generated_pages(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            second = Path(tmp) / "b.md"
            first = Path(tmp) / "a.md"
            second.write_text(
                "<!-- generated by tools/userdocs.py from tools/workflows/02_b.py -->\n# B\n"
            )
            first.write_text(
                "<!-- generated by tools/userdocs.py from tools/workflows/01_a.py -->\n# A\n"
            )
            self.assertLess(build_book._order(first), build_book._order(second))

    def test_an_authored_page_sorts_after_generated_ones(self):
        import tempfile

        with tempfile.TemporaryDirectory() as tmp:
            authored = Path(tmp) / "z.md"
            generated = Path(tmp) / "a.md"
            authored.write_text("# Z\n")
            generated.write_text(
                "<!-- generated by tools/userdocs.py from tools/workflows/09_x.py -->\n# A\n"
            )
            self.assertLess(build_book._order(generated), build_book._order(authored))


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


    def test_members_exclude_inherited_builtins(self):
        # `dir()` (and therefore the old renderer) included everything a value
        # type inherits from `int`/`Exception`. CPython point releases report
        # those differently, which is exactly what made the pages stale in CI.
        from enum import IntEnum

        class Scale(IntEnum):
            Small = 1
            Large = 2

        class Failure(Exception):
            """A kernel failure."""

            def retry(self):
                return None

        scale = [row["name"] for row in api_docs._members(Scale)]
        self.assertIn("Small", scale)
        self.assertIn("Large", scale)
        self.assertNotIn("bit_count", scale)
        self.assertNotIn("conjugate", scale)

        failure = [row["name"] for row in api_docs._members(Failure)]
        self.assertIn("retry", failure)
        self.assertNotIn("add_note", failure)
        self.assertNotIn("with_traceback", failure)


if __name__ == "__main__":
    unittest.main()
