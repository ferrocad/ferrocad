#!/usr/bin/env python3
"""Capture short animated GIFs of the desktop shell's pointer interactions.

The still figures in :mod:`ui_shots` show a control at rest. These GIFs show the
gesture: a tool switch, a Body/Face/Edge pick, a Push/Pull drag, a Fillet scrub
and a wheel flick. They run the Parametric app in a private Xephyr server,
synthesise the pointer with XTEST, capture the window with ``xwd`` between
events, and draw a synthetic cursor and a caption on each frame.

The browser host cannot supply these yet: it has no Part Push/Pull or Fillet
manipulator, and its query harness takes a final screenshot rather than a frame
sequence. See ``docs/developer/workbenches/screenshot-tool.md``.

Usage::

    python3 tools/ui_gifs.py                       # every GIF
    python3 tools/ui_gifs.py --section pushpull fillet
    python3 tools/ui_gifs.py --section tools --keep-frames
    python3 tools/ui_gifs.py --out-dir docs/user/images

The output is ``ui-<section>.gif``. Frames are manual artefacts, like the
highlighted stills: they depend on the app, the X server and the font stack, so
they are not pinned by ``userdocs.py --check``.
"""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import screenshot as shot
import ui_shots as stills

REPO = Path(__file__).resolve().parents[1]
DEFAULT_APP = REPO / "target" / "debug" / "ferrocad_parametric"
DEFAULT_WINDOW = (1600, 1000)
DEFAULT_SCREEN = (1800, 1200)

#: One entry per GIF.
#:
#: * ``crop`` is the region of the 1600×1000 window to keep.
#: * ``scale`` resizes the crop (the Fillet close-up is enlarged 2×).
#: * ``fps`` is the default frame duration.
#: * ``steps`` is the action list. Each action captures one frame:
#:   ``wait``, ``key``, ``click``, ``right_click``, ``right_press``,
#:   ``right_release``, ``move``, ``press``, ``release`` and ``drag`` (a list
#:   of points while the button is held). ``wait`` is the post-action settle
#:   before the frame; ``caption`` is carried forward to later frames.
GIFS = {
    "tools": {
        "crop": (0, 250, 1360, 730),
        "scale": 0.6,
        "fps": 5,
        "steps": [
            {"caption": "1 · Move", "click": (900, 450), "wait": 0.7, "duration": 1.0},
            {"caption": "1 · Rotate (press 1 again)", "key": "1", "wait": 0.6, "duration": 0.9},
            {"caption": "2 · Push/Pull", "key": "2", "wait": 0.4, "duration": 0.4},
            {"click": (1080, 400), "wait": 0.7, "duration": 1.0},
            {"caption": "3 · Fillet", "key": "3", "wait": 0.4, "duration": 0.4},
            {"click": (960, 340), "wait": 0.7, "duration": 1.0},
            {"caption": "0 · Inspect", "key": "0", "wait": 0.4, "duration": 0.4},
            {"click": (900, 450), "wait": 0.7, "duration": 1.0},
        ],
    },
    "pick": {
        "crop": (0, 0, 1300, 1000),
        "scale": 0.55,
        "fps": 5,
        "steps": [
            {"caption": "Inspect · Body", "key": "0", "wait": 0.4, "duration": 0.4},
            {"click": (900, 450), "wait": 0.7, "duration": 1.1},
            {"caption": "Inspect · Face", "click": (85, 912), "wait": 0.4, "duration": 0.5},
            {"click": (1080, 400), "wait": 0.7, "duration": 1.1},
            {"caption": "Inspect · Edge", "click": (118, 912), "wait": 0.4, "duration": 0.5},
            {"click": (960, 340), "wait": 0.7, "duration": 1.1},
        ],
    },

    "pushpull": {
        "crop": (700, 160, 620, 500),
        "fps": 6,
        "steps": [
            {"caption": "Push/Pull · click a face", "key": "2", "wait": 0.5, "duration": 0.5},
            {"click": (1080, 400), "wait": 0.7, "duration": 0.9},
            {"caption": "Drag the amber stem", "move": (1080, 465), "wait": 0.3, "duration": 0.4},
            {"press": (1080, 465), "duration": 0.4},
            {"drag": [(1095, 473), (1110, 481), (1125, 489), (1140, 497), (1155, 505)]},
            {"caption": "Release to commit", "release": (1155, 505), "wait": 0.9, "duration": 1.5},
        ],
    },
    "fillet": {
        "crop": (980, 260, 300, 200),
        "scale": 2.0,
        "fps": 6,
        "steps": [
            {"caption": "Fillet · select the top-right edge", "key": "3", "wait": 0.5, "duration": 0.5},
            {"click": (1062, 340), "wait": 0.7, "duration": 0.9},
            {"caption": "Drag the radius arc", "move": (1080, 357), "wait": 0.3, "duration": 0.4},
            {"press": (1080, 357), "duration": 0.4},
            {"drag": [(1086, 362), (1092, 367), (1098, 372), (1104, 377)]},
            {"caption": "Release to commit", "release": (1104, 377), "wait": 0.9, "duration": 1.5},
        ],
    },
    "wheel": {
        "crop": (620, 260, 480, 380),
        "scale": 1.5,
        "fps": 5,
        "steps": [
            {"caption": "Right-press for the object verbs", "click": (900, 450), "wait": 0.6, "duration": 0.7},
            {"right_press": (900, 450), "wait": 0.5, "duration": 0.9},
            {"caption": "Flick to a wedge", "move": (972, 410), "wait": 0.6, "duration": 0.8},
            {"caption": "Release on Rotate", "right_release": (972, 410), "wait": 0.6, "duration": 0.8},
            {"caption": "Click the body to re-arm the rings", "click": (900, 450), "wait": 0.8, "duration": 1.4},
        ],
    },
}


def _launch(display: str, args) -> tuple[subprocess.Popen, str, int, int]:
    """Start the app on ``display`` and return its process, window and origin."""
    app = stills._app_binary(Path(args.app_bin))
    env = dict(os.environ)
    env["DISPLAY"] = display
    env["WAYLAND_DISPLAY"] = ""
    env["XDG_SESSION_TYPE"] = "x11"
    proc = subprocess.Popen(
        [str(app)],
        cwd=REPO,
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    window = shot._wait_for_window(
        display, "FerroCAD", args.wait, proc, Path("/tmp/ui-gifs.log")
    )
    wid, name, *_ = window
    time.sleep(1.0)
    shot._resize_window(display, wid, list(args.window))
    time.sleep(2.0)
    window = shot._find_window(display, name) or window
    wid, name, width, height, x, y = window
    if (width, height) != tuple(args.window):
        raise RuntimeError(
            f"the app window is {width}x{height}, expected "
            f"{args.window[0]}x{args.window[1]}; increase --screen"
        )
    return proc, wid, x, y


def _focus(display: str, wid: str):
    from Xlib import X, display as xdisplay

    xd = xdisplay.Display(display)
    win = xd.create_resource_object("window", int(wid, 16))
    win.set_input_focus(X.RevertToParent, X.CurrentTime)
    xd.sync()
    return xd, win


def _button(display, root, x: int, y: int, button: int, press: bool) -> None:
    from Xlib import X
    from Xlib.ext import xtest

    root.warp_pointer(x, y)
    display.sync()
    time.sleep(0.12)
    kind = X.ButtonPress if press else X.ButtonRelease
    xtest.fake_input(display, kind, button)
    display.sync()


def _cursor(draw, x: float, y: float, pressed: bool) -> None:
    points = [
        (x, y),
        (x, y + 19),
        (x + 5, y + 14),
        (x + 9, y + 22),
        (x + 13, y + 20),
        (x + 9, y + 12),
        (x + 16, y + 12),
    ]
    fill = (255, 96, 96) if pressed else (250, 250, 250)
    draw.polygon(points, fill=fill, outline=(0, 0, 0))
    if pressed:
        draw.ellipse((x - 7, y - 7, x + 7, y + 7), outline=(72, 229, 255), width=2)


def _caption(draw, text: str) -> None:
    font = stills._font(20)
    width = draw.textlength(text, font=font)
    draw.rounded_rectangle(
        (12, 12, 12 + width + 24, 12 + 36), radius=8, fill=(15, 20, 28, 235)
    )
    draw.text((24, 18), text, fill=(72, 229, 255), font=font)


def _render(
    raw: Path,
    spec: dict,
    pointer: tuple[int, int] | None,
    pressed: bool,
    caption: str | None,
):
    from PIL import Image, ImageDraw

    with Image.open(raw) as opened:
        image = opened.convert("RGB")
    x, y, width, height = spec["crop"]
    image = image.crop((x, y, x + width, y + height))
    scale = spec.get("scale", 1.0)
    if scale != 1.0:
        image = image.resize(
            (round(width * scale), round(height * scale)),
            Image.Resampling.LANCZOS,
        )
    draw = ImageDraw.Draw(image)
    if pointer is not None:
        px = (pointer[0] - x) * scale
        py = (pointer[1] - y) * scale
        _cursor(draw, px, py, pressed)
    if caption:
        _caption(draw, caption)
    return image


def _save(frames: list, durations: list[int], out: Path) -> None:
    from PIL import Image

    palette = frames[0].quantize(colors=128, method=Image.Quantize.MEDIANCUT)
    indexed = [
        frame.quantize(palette=palette, dither=Image.Dither.NONE) for frame in frames
    ]
    out.parent.mkdir(parents=True, exist_ok=True)
    indexed[0].save(
        out,
        save_all=True,
        append_images=indexed[1:],
        duration=durations,
        loop=0,
        optimize=True,
        disposal=2,
    )


def capture(section: str, spec: dict, args, display: str) -> Path:
    from Xlib import X

    proc, wid, x, y = _launch(display, args)
    xd, win = _focus(display, wid)
    root = xd.screen().root
    frames: list = []
    durations: list[int] = []
    pointer: tuple[int, int] | None = None
    pressed = False
    caption: str | None = None
    default = int(1000 / spec.get("fps", 6))

    with tempfile.TemporaryDirectory(prefix="ferrocad-gif-") as tmp:
        tmp_path = Path(tmp)
        frame_dir = Path(args.frames_dir) / section if args.keep_frames else None
        if frame_dir:
            frame_dir.mkdir(parents=True, exist_ok=True)

        def snap(hold: float | None = None) -> None:
            raw = tmp_path / f"frame-{len(frames):03d}.png"
            shot._capture_window(display, wid, raw, None)
            # `xwd` can move the input focus back to the root, so restore it
            # before the next synthetic key or button event.
            win.set_input_focus(X.RevertToParent, X.CurrentTime)
            xd.sync()
            image = _render(raw, spec, pointer, pressed, caption)
            frames.append(image)
            durations.append(int(hold * 1000) if hold else default)
            if frame_dir:
                image.save(frame_dir / f"{len(frames) - 1:03d}.png")

        try:
            if spec.get("intro", True):
                # Wake the viewport after the resize so its camera fit matches
                # the 1600×1000 session; without an input event the first frame
                # can keep the fit from the smaller startup window.
                root.warp_pointer(
                    x + args.window[0] // 2, y + args.window[1] // 2
                )
                xd.sync()
                time.sleep(0.8)
                snap(hold=0.8)
            for step in spec["steps"]:
                if step.get("caption"):
                    caption = step["caption"]
                if "key" in step:
                    stills._send_key(xd, step["key"])
                    time.sleep(step.get("wait", 0.35))
                elif "click" in step:
                    px, py = step["click"]
                    pointer = (px, py)
                    stills._send_click(xd, root, x + px, y + py)
                    time.sleep(step.get("wait", 0.5))
                elif "right_click" in step:
                    px, py = step["right_click"]
                    pointer = (px, py)
                    _button(xd, root, x + px, y + py, 3, True)
                    time.sleep(0.05)
                    _button(xd, root, x + px, y + py, 3, False)
                    time.sleep(step.get("wait", 0.5))
                elif "right_press" in step:
                    px, py = step["right_press"]
                    pointer = (px, py)
                    _button(xd, root, x + px, y + py, 3, True)
                    pressed = True
                    time.sleep(step.get("wait", 0.4))
                elif "right_release" in step:
                    px, py = step["right_release"]
                    pointer = (px, py)
                    _button(xd, root, x + px, y + py, 3, False)
                    pressed = False
                    time.sleep(step.get("wait", 0.8))
                elif "move" in step:
                    px, py = step["move"]
                    pointer = (px, py)
                    root.warp_pointer(x + px, y + py)
                    xd.sync()
                    time.sleep(step.get("wait", 0.3))
                elif "press" in step:
                    px, py = step["press"]
                    pointer = (px, py)
                    _button(xd, root, x + px, y + py, 1, True)
                    pressed = True
                    time.sleep(step.get("wait", 0.2))
                elif "release" in step:
                    px, py = step["release"]
                    pointer = (px, py)
                    _button(xd, root, x + px, y + py, 1, False)
                    pressed = False
                    time.sleep(step.get("wait", 0.8))
                elif "drag" in step:
                    for px, py in step["drag"]:
                        pointer = (px, py)
                        root.warp_pointer(x + px, y + py)
                        xd.sync()
                        time.sleep(0.2)
                        snap(hold=0.12)
                    continue
                elif "wait" in step:
                    time.sleep(step["wait"])
                else:
                    raise RuntimeError(f"unknown step {step!r}")
                snap(hold=step.get("duration"))
        finally:
            xd.close()
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()

    out = Path(args.out_dir) / f"ui-{section}.gif"
    _save(frames, durations, out)
    return out


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--section",
        nargs="+",
        choices=("all", *GIFS),
        default=["all"],
        help="which GIFs to capture",
    )
    parser.add_argument("--out-dir", default="docs/user/images")
    parser.add_argument("--app-bin", default=str(DEFAULT_APP))
    parser.add_argument("--window", type=int, nargs=2, default=DEFAULT_WINDOW)
    parser.add_argument("--screen", type=int, nargs=2, default=DEFAULT_SCREEN)
    parser.add_argument("--xephyr", default="Xephyr")
    parser.add_argument("--parent-display", default=os.environ.get("DISPLAY"))
    parser.add_argument("--wait", type=float, default=20.0)
    parser.add_argument(
        "--frames-dir",
        default="target/gif-frames",
        help="where --keep-frames writes the processed frames",
    )
    parser.add_argument(
        "--keep-frames",
        action="store_true",
        help="also write the cropped frames under --frames-dir",
    )
    args = parser.parse_args(argv)

    if not args.parent_display:
        print("error: no parent X display; set DISPLAY", file=sys.stderr)
        return 1
    if not Path(args.app_bin).is_file():
        print(
            f"error: {args.app_bin} is not built; run `cargo build -p "
            "ferrocad_parametric`",
            file=sys.stderr,
        )
        return 1
    if shutil.which(args.xephyr) is None:
        print(f"error: {args.xephyr} is not on PATH", file=sys.stderr)
        return 1

    sections = list(GIFS) if "all" in args.section else args.section
    display, xephyr = stills._start_xephyr(args.parent_display, args.screen, args.xephyr)
    try:
        for section in sections:
            out = capture(section, GIFS[section], args, display)
            print(f"wrote {out}")
    finally:
        xephyr.terminate()
        try:
            xephyr.wait(timeout=5)
        except subprocess.TimeoutExpired:
            xephyr.kill()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
