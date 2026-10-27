"""Tests for the standalone screenshot CLI.

Hermetic where possible: the renderer is a tiny shell script, so the tests exercise
the spec/CLI/workflow plumbing without a GPU. The workflow integration test runs the
real toy workflow against the facade.

Run::

    PYTHONPATH=python python3 tools/test_screenshot.py
"""

import os
import struct
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

TOOLS = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(TOOLS)

sys.path.insert(0, TOOLS)
sys.path.insert(0, os.path.join(REPO, "python"))

import screenshot  # noqa: E402


class TestSize(unittest.TestCase):
    def test_a_size_parses(self):
        self.assertEqual(screenshot.parse_size("1440x900"), [1440, 900])

    def test_a_bad_size_is_rejected(self):
        for value in ("1440", "1440x", "axb", "0x100", "100x-1"):
            with self.assertRaises(Exception, msg=value):
                screenshot.parse_size(value)


class TestRegion(unittest.TestCase):
    def test_a_region_parses(self):
        self.assertEqual(screenshot.parse_region("10,20,300,400"), [10, 20, 300, 400])

    def test_a_bad_region_is_rejected(self):
        for value in ("10,20,300", "a,20,300,400", "10,20,0,400", "10,20,300,-1"):
            with self.assertRaises(Exception, msg=value):
                screenshot.parse_region(value)


class TestLoadSpec(unittest.TestCase):
    def test_a_spec_is_a_json_object(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp, "spec.json")
            path.write_text('{"camera": {}, "shapes": []}')
            self.assertEqual(screenshot.load_spec(path), {"camera": {}, "shapes": []})

    def test_invalid_json_says_where(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp, "spec.json")
            path.write_text("{")
            with self.assertRaises(RuntimeError) as caught:
                screenshot.load_spec(path)
            self.assertIn(str(path), str(caught.exception))

    def test_a_non_object_spec_is_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp, "spec.json")
            path.write_text("[]")
            with self.assertRaises(RuntimeError):
                screenshot.load_spec(path)


class TestRenderSpec(unittest.TestCase):
    def fake_binary(self, tmp, body):
        path = Path(tmp, "renderer.sh")
        path.write_text("#!/bin/sh\n" + body)
        path.chmod(0o755)
        return path

    def test_the_renderer_receives_the_spec_and_the_output(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary = self.fake_binary(tmp, 'cp "$1" "$2"\n')
            out = Path(tmp, "out.png")
            code = screenshot.render_spec({"camera": {}, "shapes": []}, out, binary)
            self.assertEqual(code, 0)
            self.assertEqual(out.read_text(), '{\n  "camera": {},\n  "shapes": []\n}\n')

    def test_no_adapter_is_exit_three(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary = self.fake_binary(tmp, "exit 3\n")
            out = Path(tmp, "out.png")
            self.assertEqual(
                screenshot.render_spec({"camera": {}, "shapes": []}, out, binary), 3
            )
            self.assertFalse(out.exists())

    def test_a_render_error_is_exit_one(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary = self.fake_binary(tmp, "echo bad >&2; exit 2\n")
            out = Path(tmp, "out.png")
            self.assertEqual(
                screenshot.render_spec({"camera": {}, "shapes": []}, out, binary), 1
            )
            self.assertFalse(out.exists())


class TestXwd(unittest.TestCase):
    """The small XWD reader, without a display."""

    @staticmethod
    def xwd(pixels, *, byte_order=0, width=2, height=2):
        # A 32-bpp TrueColor ZPixmap header, then one little/big-endian word per
        # pixel. The masks are the usual 0x00RRGGBB.
        header = struct.pack(
            ">25I",
            100, 7, 2, 24, width, height, 0, byte_order, 32, 0, 32, 32,
            width * 4, 4, 0xFF0000, 0x00FF00, 0x0000FF, 8, 0, 0,
            width, height, 0, 0, 0,
        )
        endian = "little" if byte_order == 0 else "big"
        body = b"".join(pixel.to_bytes(4, endian) for pixel in pixels)
        return header + body, width, height

    def test_a_lsb_truecolor_image_decodes(self):
        data, width, height = self.xwd(
            [0x00FF0000, 0x0000FF00, 0x000000FF, 0x00FFFFFF]
        )
        w, h, rgba = screenshot._xwd_to_rgba(data, None)
        self.assertEqual((w, h), (width, height))
        self.assertEqual(rgba[:4], bytes([255, 0, 0, 255]))
        self.assertEqual(rgba[4:8], bytes([0, 255, 0, 255]))
        self.assertEqual(rgba[8:12], bytes([0, 0, 255, 255]))
        self.assertEqual(rgba[12:16], bytes([255, 255, 255, 255]))

    def test_an_msb_truecolor_image_decodes(self):
        data, _width, _height = self.xwd([0x00FF0000], byte_order=1, width=1, height=1)
        _w, _h, rgba = screenshot._xwd_to_rgba(data, None)
        self.assertEqual(rgba[:4], bytes([255, 0, 0, 255]))

    def test_a_region_crops(self):
        data, _width, _height = self.xwd(
            [0x00FF0000, 0x0000FF00, 0x000000FF, 0x00FFFFFF]
        )
        w, h, rgba = screenshot._xwd_to_rgba(data, [1, 0, 1, 2])
        self.assertEqual((w, h), (1, 2))
        self.assertEqual(rgba[:4], bytes([0, 255, 0, 255]))
        self.assertEqual(rgba[4:8], bytes([255, 255, 255, 255]))

    def test_a_truncated_header_is_rejected(self):
        with self.assertRaises(RuntimeError):
            screenshot._xwd_to_rgba(b"short", None)

    def test_the_png_writer_emits_a_deterministic_rgba_png(self):
        with tempfile.TemporaryDirectory() as tmp:
            first = Path(tmp, "a.png")
            second = Path(tmp, "b.png")
            rgba = bytes([255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255])
            screenshot._write_png(first, 2, 2, rgba)
            screenshot._write_png(second, 2, 2, rgba)
            self.assertEqual(first.read_bytes()[:8], b"\x89PNG\r\n\x1a\n")
            self.assertEqual(first.read_bytes(), second.read_bytes())
            self.assertEqual(struct.unpack(">I", first.read_bytes()[16:20])[0], 2)
            self.assertEqual(struct.unpack(">I", first.read_bytes()[20:24])[0], 2)


class TestWindowTree(unittest.TestCase):
    def test_the_window_matcher_reads_id_and_geometry(self):
        tree = (
            "xwininfo: Window id: 0x1 (the root window)\n\n"
            '  0x2 "FerroCAD: Toy": ()  1180x760+80+80  +80+80\n'
        )
        with mock.patch.object(screenshot, "_window_tree", return_value=tree):
            window = screenshot._find_window(":0", "FerroCAD")
        self.assertEqual(window, ("0x2", "FerroCAD: Toy", 1180, 760, 80, 80))

    def test_an_unrelated_window_is_ignored(self):
        tree = '  0x2 "Something else": ()  100x100+0+0  +0+0\n'
        with mock.patch.object(screenshot, "_window_tree", return_value=tree):
            self.assertIsNone(screenshot._find_window(":0", "FerroCAD"))

    def test_the_largest_matching_window_wins(self):
        tree = (
            '  0x2 "FerroCAD Dev Tools": ()  420x760+1300+80  +1300+80\n'
            '  0x3 "FerroCAD: Toy": ()  1180x760+80+80  +80+80\n'
        )
        with mock.patch.object(screenshot, "_window_tree", return_value=tree):
            window = screenshot._find_window(":0", "FerroCAD")
        self.assertEqual(window[0], "0x3")


class TestDisplayModes(unittest.TestCase):
    def args(self, **overrides):
        base = dict(display=None, size=None)
        base.update(overrides)
        return SimpleNamespace(**base)

    def test_chrome_without_a_display_or_xvfb_names_the_requirement(self):
        with mock.patch.dict(os.environ, {"DISPLAY": ""}), mock.patch(
            "shutil.which", return_value=None
        ):
            with self.assertRaises(RuntimeError) as caught:
                screenshot._ensure_display(self.args(), allow_xvfb=True)
        self.assertIn("no DISPLAY and Xvfb", str(caught.exception))

    def test_desktop_without_a_display_is_rejected(self):
        with mock.patch.dict(os.environ, {"DISPLAY": ""}):
            with self.assertRaises(RuntimeError) as caught:
                screenshot._ensure_display(self.args(), allow_xvfb=False)
        self.assertIn("desktop capture needs an X display", str(caught.exception))

    def test_desktop_rejects_a_size(self):
        with self.assertRaises(SystemExit):
            screenshot.main(
                ["--mode", "desktop", "--out", "x.png", "--size", "1280x720"]
            )

    def test_chrome_rejects_a_region(self):
        with self.assertRaises(SystemExit):
            screenshot.main(
                ["--mode", "chrome", "--out", "x.png", "--region", "0,0,10,10"]
            )


class TestWaylandDesktop(unittest.TestCase):
    def test_grim_is_used_when_present(self):
        with tempfile.TemporaryDirectory() as tmp:
            grim = Path(tmp, "grim")
            grim.write_text('#!/bin/sh\nfor last; do :; done\nprintf png > "$last"\n')
            grim.chmod(0o755)
            out = Path(tmp, "desktop.png")
            with mock.patch("shutil.which", return_value=str(grim)):
                self.assertTrue(screenshot._grim_screenshot(out, None))
            self.assertEqual(out.read_bytes(), b"png")

    def test_grim_receives_a_region(self):
        with tempfile.TemporaryDirectory() as tmp:
            log = Path(tmp, "args")
            grim = Path(tmp, "grim")
            grim.write_text(f'#!/bin/sh\nprintf "%s" "$*" > {log}\n')
            grim.chmod(0o755)
            out = Path(tmp, "desktop.png")
            with mock.patch("shutil.which", return_value=str(grim)):
                self.assertTrue(screenshot._grim_screenshot(out, [1, 2, 30, 40]))
            self.assertIn("-g 1,2 30x40", log.read_text())

    def test_a_wayland_session_falls_back_to_grim(self):
        with tempfile.TemporaryDirectory() as tmp:
            grim = Path(tmp, "grim")
            grim.write_text('#!/bin/sh\nfor last; do :; done\nprintf png > "$last"\n')
            grim.chmod(0o755)
            out = Path(tmp, "desktop.png")
            args = SimpleNamespace(display=None, out=str(out), region=None)
            with mock.patch.dict(
                os.environ,
                {"DISPLAY": "", "WAYLAND_DISPLAY": "wayland-0", "XDG_SESSION_TYPE": "wayland"},
            ), mock.patch("shutil.which", return_value=str(grim)):
                self.assertEqual(screenshot.capture_desktop(args), 0)
            self.assertEqual(out.read_bytes(), b"png")

    def test_no_display_and_no_wayland_is_a_clear_error(self):
        args = SimpleNamespace(display=None, out="target/x.png", region=None)
        with mock.patch.dict(
            os.environ, {"DISPLAY": "", "WAYLAND_DISPLAY": "", "XDG_SESSION_TYPE": ""}
        ):
            with self.assertRaises(RuntimeError) as caught:
                screenshot.capture_desktop(args)
        self.assertIn("needs an X display or a Wayland session", str(caught.exception))


class TestCompose(unittest.TestCase):
    def test_two_captures_are_composed_side_by_side(self):
        try:
            from PIL import Image
        except ImportError:
            self.skipTest("Pillow is not installed")
        with tempfile.TemporaryDirectory() as tmp:
            left = Path(tmp, "left.png")
            right = Path(tmp, "right.png")
            out = Path(tmp, "out.png")
            Image.new("RGB", (40, 20), (255, 0, 0)).save(left)
            Image.new("RGB", (10, 30), (0, 0, 255)).save(right)
            screenshot._compose_side_by_side(left, right, out, gap=5)
            with Image.open(out) as composed:
                self.assertEqual(composed.size, (55, 30))
                self.assertEqual(composed.getpixel((0, 0)), (255, 0, 0))
                self.assertEqual(composed.getpixel((50, 29)), (0, 0, 255))
                self.assertEqual(composed.getpixel((42, 10)), (15, 18, 24))

    def test_the_gap_defaults_to_twelve_pixels(self):
        try:
            from PIL import Image
        except ImportError:
            self.skipTest("Pillow is not installed")
        with tempfile.TemporaryDirectory() as tmp:
            left = Path(tmp, "left.png")
            right = Path(tmp, "right.png")
            out = Path(tmp, "out.png")
            Image.new("RGB", (4, 4), (1, 2, 3)).save(left)
            Image.new("RGB", (4, 4), (4, 5, 6)).save(right)
            screenshot._compose_side_by_side(left, right, out)
            with Image.open(out) as composed:
                self.assertEqual(composed.size, (20, 4))

    def test_missing_pillow_says_what_is_needed(self):
        with mock.patch.dict(sys.modules, {"PIL": None, "PIL.Image": None}):
            with self.assertRaises(RuntimeError) as caught:
                screenshot._compose_side_by_side(Path("a"), Path("b"), Path("c"))
        self.assertIn("Pillow", str(caught.exception))


class TestDevToolsGuard(unittest.TestCase):
    def test_dev_tools_is_chrome_only(self):
        with self.assertRaises(SystemExit):
            screenshot.main(
                ["--mode", "viewport", "--dev-tools", "--workflow", "toy-workbench"]
            )
        with self.assertRaises(SystemExit):
            screenshot.main(
                ["--mode", "desktop", "--dev-tools", "--out", "x.png"]
            )

    def test_chrome_accepts_dev_tools(self):
        with mock.patch.object(screenshot, "capture_chrome", return_value=0) as chrome:
            self.assertEqual(
                screenshot.main(
                    ["--mode", "chrome", "--dev-tools", "--out", "x.png"]
                ),
                0,
            )
        self.assertTrue(chrome.call_args.args[0].dev_tools)


class TestWorkflow(unittest.TestCase):
    def test_a_slug_resolves_to_its_workflow(self):
        path = screenshot.workflow_path("toy-workbench")
        self.assertEqual(path.name, "04_toy_workbench.py")

    def test_an_unknown_slug_is_reported(self):
        with self.assertRaises(RuntimeError):
            screenshot.workflow_path("not-a-workflow")

    def test_render_workflow_writes_each_image_op(self):
        with tempfile.TemporaryDirectory() as tmp:
            binary = Path(tmp, "renderer.sh")
            binary.write_text('#!/bin/sh\ncp "$1" "$2"\n')
            binary.chmod(0o755)
            out_dir = Path(tmp, "shots")
            code = screenshot.render_workflow("toy-workbench", out_dir, binary)
            self.assertEqual(code, 0)
            # The fake renderer copies the spec, so the file is JSON rather than a PNG.
            self.assertIn("camera", (out_dir / "toy-slab.png").read_text())


if __name__ == "__main__":
    unittest.main()
