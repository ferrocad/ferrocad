"""Tests for the generated-documentation tooling.

Hermetic: exercises discovery and rendering with synthetic captures, so it needs
no GPU, no workflows, and no committed pages.

Run::

    PYTHONPATH=python python3 tools/test_docs_tooling.py
"""

import os
import re
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

            # The snapshot tool's exit code 3 is "no GPU adapter".
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


class TestImageManifest(unittest.TestCase):
    """The GPU-free half of the screenshot guarantee."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.image_dir = Path(self.temp.name, "images")
        self.image_dir.mkdir()
        self.patch = mock.patch.object(userdocs, "IMAGE_DIR", self.image_dir)
        self.patch.start()

    def tearDown(self):
        self.patch.stop()
        self.temp.cleanup()

    @staticmethod
    def spec(*, eye=(-120.0, -160.0, 120.0), name="demo.png"):
        return {
            "size": [960, 600],
            "camera": {"eye": list(eye), "target": [0.0, 0.0, 0.0], "up": [0.0, 0.0, 1.0]},
            "shapes": [
                {
                    "geometry": {"box": {"size": [10.0, 8.0, 6.0], "origin": [0.0, 0.0, 0.0]}},
                    "draw_mode": "shaded_with_edges",
                }
            ],
        }

    def image(self, *, name="demo.png", spec=None, slug="demo"):
        return (
            Path("01_demo.py"),
            capture(
                slug=slug,
                ops=[
                    {
                        "kind": "image",
                        "name": name,
                        "alt": "a box",
                        "caption": None,
                        "spec": self.spec() if spec is None else spec,
                    }
                ],
            ),
        )

    def write_bytes(self, name="demo.png", data=b"png bytes"):
        path = self.image_dir / name
        path.write_bytes(data)
        return path

    def test_a_spec_digest_ignores_key_order(self):
        first = {"camera": {"eye": [1, 2, 3], "target": [0, 0, 0]}, "shapes": []}
        second = {"shapes": [], "camera": {"target": [0, 0, 0], "eye": [1, 2, 3]}}
        self.assertEqual(userdocs.spec_digest(first), userdocs.spec_digest(second))

    def test_the_manifest_records_the_spec_and_the_png(self):
        self.write_bytes()
        changed, count = userdocs.write_manifest([self.image()])
        self.assertTrue(changed)
        self.assertEqual(count, 1)

        manifest = userdocs.load_manifest()
        entry = manifest["images"]["demo.png"]
        self.assertEqual(entry["workflows"], ["demo"])
        self.assertEqual(entry["bytes"], len(b"png bytes"))
        self.assertEqual(
            entry["spec_sha256"],
            userdocs.spec_digest(self.spec()),
        )
        self.assertEqual(userdocs.check_images([self.image()]), [])
        self.assertFalse(userdocs.write_manifest([self.image()])[0])

    def test_a_changed_spec_is_stale_even_without_a_gpu(self):
        self.write_bytes()
        userdocs.write_manifest([self.image()])
        changed = self.image(spec=self.spec(eye=(-200.0, -160.0, 120.0)))
        errors = userdocs.check_images([changed])
        self.assertEqual(len(errors), 1)
        self.assertIn("stale spec: demo.png", errors[0])

    def test_a_changed_png_is_stale(self):
        self.write_bytes()
        userdocs.write_manifest([self.image()])
        self.write_bytes(data=b"edited bytes")
        errors = userdocs.check_images([self.image()])
        self.assertEqual(len(errors), 1)
        self.assertIn("stale image: demo.png", errors[0])

    def test_a_missing_png_is_reported(self):
        self.write_bytes()
        userdocs.write_manifest([self.image()])
        (self.image_dir / "demo.png").unlink()
        errors = userdocs.check_images([self.image()])
        self.assertEqual(len(errors), 1)
        self.assertIn("missing image: demo.png", errors[0])

    def test_an_orphan_manifest_entry_is_reported(self):
        self.write_bytes()
        userdocs.write_manifest([self.image()])
        errors = userdocs.check_images([])
        self.assertEqual(len(errors), 1)
        self.assertIn("orphan manifest entry: demo.png", errors[0])

    def test_a_missing_manifest_is_reported_for_generated_images(self):
        self.write_bytes()
        errors = userdocs.check_images([self.image()])
        self.assertEqual(len(errors), 1)
        self.assertIn("missing docs/user/images/manifest.json", errors[0])

    def test_write_manifest_refuses_a_spec_without_a_png(self):
        with self.assertRaises(RuntimeError):
            userdocs.write_manifest([self.image()])

    def test_two_workflows_may_share_an_image_only_with_the_same_spec(self):
        first = self.image(slug="a")
        second = self.image(slug="b")
        entries = userdocs.expected_images([first, second])
        self.assertEqual(entries["demo.png"]["workflows"], ["a", "b"])

        other = self.image(slug="b", spec=self.spec(eye=(-200.0, -160.0, 120.0)))
        with self.assertRaises(RuntimeError):
            userdocs.expected_images([first, other])

    def test_a_manual_image_is_not_in_the_manifest(self):
        manual = (
            Path("01_demo.py"),
            capture(
                ops=[
                    {
                        "kind": "image",
                        "name": "manual.png",
                        "alt": "a screenshot",
                        "caption": None,
                        "spec": None,
                    }
                ]
            ),
        )
        self.assertEqual(userdocs.expected_images([manual]), {})
        changed, count = userdocs.write_manifest([manual])
        self.assertTrue(changed)
        self.assertEqual(count, 0)
        self.assertEqual(userdocs.check_images([manual]), [])


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


class TestDocumentIndex(unittest.TestCase):
    """Every note under docs/ is listed once in docs/INDEX.md."""

    INDEX = Path(REPO, "docs", "INDEX.md")
    DOCS = Path(REPO, "docs")

    @staticmethod
    def _entries(text):
        # An entry is a bullet whose first link is the note itself; prose links and
        # the "where do I start" table are deliberately not counted.
        pattern = re.compile(r"^\s*- \[[^\]]*\]\(([^)]+)\)")
        for line in text.splitlines():
            match = pattern.match(line)
            if not match:
                continue
            target = match.group(1).split("#", 1)[0].strip()
            if target and "://" not in target:
                yield target

    def test_every_markdown_note_is_listed_once(self):
        targets = list(self._entries(self.INDEX.read_text()))
        resolved = [(self.DOCS / target).resolve() for target in targets]
        expected = {
            path.resolve()
            for path in self.DOCS.rglob("*.md")
            if path.resolve() != self.INDEX.resolve()
        }
        self.assertEqual(len(resolved), len(set(resolved)), "a note is listed twice")
        self.assertEqual(set(resolved) - expected, set(), "the index lists a non-note")
        self.assertEqual(expected - set(resolved), set(), "a note is missing from the index")

    def test_the_index_names_the_six_folders(self):
        text = self.INDEX.read_text()
        for heading in (
            "## General concepts",
            "## User documentation",
            "## Mods documentation",
            "## Python API reference",
            "## Developer documentation",
            "## Project documentation",
        ):
            self.assertIn(heading, text)


class TestBook(unittest.TestCase):
    def test_a_link_that_leaves_the_book_becomes_a_github_url(self):
        source = build_book.USER / "explanation" / "x.md"
        out = build_book._rewrite_links("[x](../../developer/workbenches/conformance.md)", source)
        self.assertIn(
            "https://github.com/ferrocad/ferrocad/blob/main/docs/developer/workbenches/conformance.md",
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
        self.assertEqual(
            build_book._book_path(
                build_book.GENERAL / "01-the-dna-of-3d-geometry.md"
            ).as_posix(),
            "general/01-the-dna-of-3d-geometry.md",
        )

    def test_a_general_link_stays_inside_the_book(self):
        source = build_book.USER / "explanation" / "x.md"
        out = build_book._rewrite_links(
            "[x](../../general/01-the-dna-of-3d-geometry.md)", source
        )
        self.assertIn("(../general/01-the-dna-of-3d-geometry.md)", out)

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
