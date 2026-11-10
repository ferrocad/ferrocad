#!/usr/bin/env python3
"""Capture highlighted screenshots of the app's UI controls.

Each figure is a pair:

* a full window capture with the control marked by a cyan box and a label;
* a clean close-up cropped around the same control, scaled 2×.

Two sources feed the table:

* **web** — `ferrocad_web_gpui` in headless Chrome. The page's query harness drives
  the shell; this needs no display server and supplies the Web tab of every figure.
* **xephyr** — the Parametric app inside a private Xephyr server. Synthetic X input
  is local to that server; this supplies the Desktop tab, where the transform gizmo
  and the Part manipulators (Extrude and Fillet) are drawn.

The images are manual artefacts for the user docs. They are deliberately not in
`docs/user/images/manifest.json`: the app's window manager, GPU and font stack make
them non-deterministic, and `tools/userdocs.py --check` only pins the generated
viewport images.

Usage::

    python3 tools/ui_shots.py                    # every section (default: all)
    python3 tools/ui_shots.py --section move-desktop
    python3 tools/ui_shots.py --section wheel-menu
    python3 tools/ui_shots.py --section move-web rotate-web
    python3 tools/ui_shots.py --out-dir docs/user/images
"""

from __future__ import annotations

import argparse
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import screenshot as shot

REPO = Path(__file__).resolve().parents[1]
WEB_DIR = REPO / "crates" / "ferrocad_web_gpui" / "www"
DEFAULT_APP = REPO / "target" / "debug" / "ferrocad_parametric"
DEFAULT_WINDOW = (1600, 1000)
DEFAULT_SCREEN = (1800, 1200)

#: One entry per figure.
#:
#: * `source` is `web` or `xephyr`.
#: * `query` is the page harness URL (web only).
#: * `steps` is the X11 action list (xephyr only): `{"key": ...}`,
#:   `{"click": (x, y)}`, `{"move": (x, y)}` or `{"wait": seconds}`.
#: * `box` is the cyan highlight, `crop` the close-up, both window-relative.
SECTIONS = {
    "chrome-web": {
        "source": "web",
        "label": "Window chrome (Web)",
        "query": "?click=27,16",
        "window": (1600, 1000),
        "box": (10, 4, 230, 210),
        "crop": (6, 0, 238, 218),
        "label_side": "right",
    },
    "chrome-desktop": {
        "source": "xephyr",
        "label": "Window chrome (Desktop)",
        "steps": [
            {"click": (27, 16)},
            {"wait": 0.4},
        ],
        "box": (10, 4, 230, 210),
        "crop": (6, 0, 238, 218),
        "label_side": "right",
    },
    "wheel-menu-web": {
        "source": "web",
        "label": "Wheel menu (Web)",
        "query": "?keys=move:600,384;space",
        "window": (1600, 1000),
        "box": (480, 264, 240, 240),
        "crop": (470, 254, 260, 260),
    },
    "wheel-menu": {
        "source": "xephyr",
        "label": "Wheel menu",
        "steps": [
            {"click": (768, 450)},
            {"move": (450, 600)},
            {"key": "space"},
            {"wait": 1.0},
        ],
        "box": (320, 470, 260, 260),
        "crop": (310, 460, 280, 280),
    },
    "toolbar-web": {
        "source": "web",
        "label": "Toolbar (Web)",
        "query": "?",
        "window": (1600, 1000),
        "box": (10, 46, 548, 34),
        "crop": (8, 44, 552, 38),
        "label_below": True,
    },
    "toolbar-desktop": {
        "source": "xephyr",
        "label": "Toolbar (Desktop)",
        "steps": [],
        "box": (10, 44, 510, 36),
        "crop": (8, 42, 514, 40),
        "label_below": True,
    },
    "action-bar-web": {
        "source": "web",
        "label": "Action bar (Web)",
        "query": "?",
        "window": (1600, 1000),
        "box": (450, 930, 300, 36),
        "crop": (446, 925, 308, 46),
    },
    "action-bar-desktop": {
        "source": "xephyr",
        "label": "Action bar (Desktop)",
        "steps": [],
        "box": (608, 928, 380, 40),
        "crop": (604, 923, 388, 50),
    },
    "loadout-web": {
        "source": "web",
        "label": "Tool loadout (Web)",
        "query": "?",
        "window": (1600, 1000),
        "box": (14, 929, 362, 40),
        "crop": (10, 925, 370, 48),
    },
    "loadout-desktop": {
        "source": "xephyr",
        "label": "Tool loadout (Desktop)",
        "steps": [],
        "box": (16, 928, 358, 38),
        "crop": (12, 924, 366, 46),
    },
    "move-web": {
        "source": "web",
        "label": "Move (Web)",
        "query": "?select=600,400",
        "window": (1600, 1000),
        "box": (10, 884, 492, 38),
        "crop": (8, 880, 496, 46),
    },
    "move-desktop": {
        "source": "xephyr",
        "label": "Move (Desktop)",
        "steps": [
            {"click": (900, 450)},
            {"move": (900, 450)},
            {"wait": 0.7},
        ],
        "box": (735, 372, 180, 178),
        "crop": (722, 366, 214, 194),
    },
    "rotate-web": {
        "source": "web",
        "label": "Rotate (Web)",
        "query": "?select=600,400&keys=ctrl-shift-r",
        "window": (1600, 1000),
        "box": (10, 884, 572, 38),
        "crop": (8, 880, 576, 46),
    },
    "rotate-desktop": {
        "source": "xephyr",
        "label": "Rotate (Desktop)",
        "steps": [
            {"click": (900, 450)},
            {"wait": 0.5},
            {"key": "1"},
            {"move": (900, 450)},
            {"wait": 0.7},
        ],
        "box": (660, 380, 250, 240),
        "crop": (650, 375, 260, 250),
    },
    "extrude-web": {
        "source": "web",
        "label": "Extrude (Web)",
        "query": "?select=600,400&keys=2",
        "window": (1600, 1000),
        "box": (106, 926, 106, 42),
        "crop": (98, 919, 122, 56),
    },
    "extrude-desktop": {
        "source": "xephyr",
        "label": "Extrude (Desktop)",
        "steps": [
            {"key": "2"},
            {"click": (1080, 400)},
            {"move": (1080, 400)},
            {"wait": 0.6},
        ],
        "box": (1020, 435, 250, 65),
        "crop": (1010, 425, 270, 85),
    },
    "fillet-web": {
        "source": "web",
        "label": "Fillet (Web)",
        "query": "?select=600,400&keys=3",
        "window": (1600, 1000),
        "box": (212, 926, 76, 42),
        "crop": (204, 919, 92, 56),
    },
    "fillet-desktop": {
        "source": "xephyr",
        "label": "Fillet (Desktop)",
        "steps": [
            {"key": "3"},
            {"click": (1062, 340)},
            {"move": (1080, 347)},
            {"wait": 0.6},
        ],
        "box": (1060, 315, 105, 50),
        "crop": (1050, 308, 130, 70),
    },
}


def _start_xephyr(parent: str, screen: tuple[int, int], xephyr: str) -> tuple[str, subprocess.Popen]:
    """Start a nested X server on a free display and return it."""
    for number in range(40, 80):
        display = f":{number}"
        if os.path.exists(f"/tmp/.X11-unix/X{number}"):
            continue
        if shot._display_works(display):
            continue
        env = dict(os.environ)
        env["DISPLAY"] = parent
        proc = subprocess.Popen(
            [
                xephyr,
                display,
                "-screen",
                f"{screen[0]}x{screen[1]}x24",
                "-ac",
                "-br",
                "-noreset",
            ],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        deadline = time.monotonic() + 5.0
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                break
            if shot._display_works(display):
                return display, proc
            time.sleep(0.1)
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
    raise RuntimeError("could not start Xephyr on a free display")


def _app_binary(path: Path) -> Path:
    if path.is_file():
        return path
    raise RuntimeError(
        f"{path} is not built; run `cargo build -p ferrocad_parametric` "
        "or pass --app-bin"
    )


def _find_chrome(explicit: str) -> str:
    if explicit:
        if Path(explicit).is_file() or shutil.which(explicit):
            return explicit
        raise RuntimeError(f"--chrome={explicit} is not executable")
    for name in ("google-chrome", "google-chrome-stable", "chromium"):
        if shutil.which(name):
            return name
    raise RuntimeError("no Chrome/Chromium found; pass --chrome")


def _free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def _wait_for_port(port: int) -> None:
    deadline = time.monotonic() + 10.0
    while time.monotonic() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", port), 0.2):
                return
        except OSError:
            time.sleep(0.1)
    raise RuntimeError(f"the web server did not start on port {port}")


def _font(size: int = 18):
    from PIL import ImageFont

    for candidate in (
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    ):
        if Path(candidate).is_file():
            return ImageFont.truetype(candidate, size)
    try:
        return ImageFont.load_default(size=size)
    except TypeError:
        return ImageFont.load_default()


def _send_click(display, root, x: int, y: int, button: int = 1, pause: float = 0.05) -> None:
    from Xlib import X
    from Xlib.ext import xtest

    root.warp_pointer(x, y)
    display.sync()
    time.sleep(0.2)
    xtest.fake_input(display, X.ButtonPress, button)
    display.sync()
    time.sleep(pause)
    xtest.fake_input(display, X.ButtonRelease, button)
    display.sync()


def _send_key(display, name: str) -> None:
    from Xlib import X, XK
    from Xlib.ext import xtest

    code = display.keysym_to_keycode(XK.string_to_keysym(name))
    xtest.fake_input(display, X.KeyPress, code)
    display.sync()
    time.sleep(0.05)
    xtest.fake_input(display, X.KeyRelease, code)
    display.sync()
    time.sleep(0.4)


def _capture_xephyr(spec: dict, args, raw: Path) -> None:
    app = _app_binary(Path(args.app_bin))
    parent = args.parent_display or os.environ.get("DISPLAY")
    if not parent:
        raise RuntimeError("no parent X display; set DISPLAY or pass --parent-display")
    display, xephyr = _start_xephyr(parent, args.screen, args.xephyr)
    app_proc = None
    try:
        env = dict(os.environ)
        env["DISPLAY"] = display
        env["WAYLAND_DISPLAY"] = ""
        env["XDG_SESSION_TYPE"] = "x11"
        app_proc = subprocess.Popen(
            [str(app)],
            cwd=REPO,
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        window = shot._wait_for_window(display, "FerroCAD", args.wait, app_proc, Path("/tmp/ui-shots.log"))
        wid, name, *_ = window
        time.sleep(1.0)
        shot._resize_window(display, wid, list(args.window))
        time.sleep(2.0)
        window = shot._find_window(display, name) or window
        wid, name, width, height, x, y = window
        if (width, height) != tuple(args.window):
            raise RuntimeError(
                f"the app window is {width}x{height}, expected {args.window[0]}x{args.window[1]}; "
                "increase --screen or lower --window"
            )

        from Xlib import X, display as xdisplay

        xdisplay_obj = xdisplay.Display(display)
        root = xdisplay_obj.screen().root
        win = xdisplay_obj.create_resource_object("window", int(wid, 16))
        win.set_input_focus(X.RevertToParent, X.CurrentTime)
        xdisplay_obj.sync()

        for step in spec["steps"]:
            if "key" in step:
                _send_key(xdisplay_obj, step["key"])
            elif "click" in step:
                cx, cy = step["click"]
                _send_click(xdisplay_obj, root, x + cx, y + cy)
                time.sleep(0.5)
            elif "move" in step:
                mx, my = step["move"]
                root.warp_pointer(x + mx, y + my)
                xdisplay_obj.sync()
                time.sleep(0.2)
            elif "wait" in step:
                time.sleep(step["wait"])
            else:  # pragma: no cover - a malformed SECTIONS entry
                raise RuntimeError(f"unknown step {step!r}")
        xdisplay_obj.close()
        shot._capture_window(display, wid, raw, None)
    finally:
        if app_proc is not None:
            app_proc.terminate()
            try:
                app_proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                app_proc.kill()
                app_proc.wait(timeout=5)
        xephyr.terminate()
        try:
            xephyr.wait(timeout=5)
        except subprocess.TimeoutExpired:
            xephyr.kill()


def _capture_web(spec: dict, args, raw: Path) -> None:
    chrome = _find_chrome(args.chrome)
    web_dir = Path(args.web_dir)
    if not (web_dir / "pkg").is_dir():
        raise RuntimeError(
            f"{web_dir}/pkg is missing; build it with "
            "`crates/ferrocad_web_gpui/run.sh --build-only`"
        )
    port = args.port or _free_port()
    server = subprocess.Popen(
        [
            sys.executable,
            "-m",
            "http.server",
            str(port),
            "--bind",
            "127.0.0.1",
            "--directory",
            str(web_dir),
        ],
        cwd=REPO,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        _wait_for_port(port)
        width, height = spec.get("window", args.window)
        query = spec["query"]
        url = f"http://127.0.0.1:{port}/{query}"
        with tempfile.TemporaryDirectory(prefix="ferrocad-chrome-") as profile:
            command = [
                chrome,
                "--headless=new",
                "--no-sandbox",
                "--disable-dev-shm-usage",
                "--use-angle=swiftshader",
                "--enable-unsafe-swiftshader",
                f"--window-size={width},{height}",
                "--virtual-time-budget=20000",
                "--enable-logging=stderr",
                "--v=0",
                f"--user-data-dir={profile}",
                f"--screenshot={raw}",
                url,
            ]
            proc = subprocess.run(
                command,
                cwd=REPO,
                capture_output=True,
                text=True,
                timeout=args.wait + 180.0,
            )
            if not raw.is_file():
                tail = (proc.stderr or proc.stdout or "").strip()[-2000:]
                raise RuntimeError(
                    f"headless Chrome produced no screenshot for {url}"
                    + (f"\n{tail}" if tail else "")
                )
    finally:
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()


def _write_figures(raw: Path, full_out: Path, close_out: Path, spec: dict) -> None:
    try:
        from PIL import Image, ImageDraw
    except ImportError as exc:
        raise RuntimeError("UI shots need Pillow (`pip install pillow`)") from exc

    with Image.open(raw) as opened:
        image = opened.convert("RGB")

    x, y, width, height = spec["crop"]
    close = image.crop((x, y, x + width, y + height))
    close = close.resize((width * 2, height * 2), Image.Resampling.LANCZOS)
    close.save(close_out)

    draw = ImageDraw.Draw(image, "RGBA")
    bx, by, bw, bh = spec["box"]
    accent = (72, 229, 255, 255)
    draw.rounded_rectangle((bx, by, bx + bw, by + bh), radius=12, outline=accent, width=4)
    label = spec["label"]
    font = _font(18)
    tw = draw.textlength(label, font=font)
    if spec.get("label_side") == "right":
        lx = bx + bw + 10
        ly = by
    else:
        lx = bx
        if spec.get("label_below") or by < 34:
            ly = by + bh + 8
        else:
            ly = by - 34
    draw.rounded_rectangle((lx, ly, lx + tw + 22, ly + 30), radius=7, fill=(15, 20, 28, 235))
    draw.text((lx + 11, ly + 6), label, fill=accent, font=font)
    image.save(full_out)


def capture(section: str, args) -> tuple[Path, Path]:
    spec = SECTIONS[section]
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    full_out = out_dir / f"ui-{section}-window.png"
    close_out = out_dir / f"ui-{section}.png"
    with tempfile.TemporaryDirectory(prefix="ferrocad-ui-shots-") as tmp:
        raw = Path(tmp) / "raw.png"
        if spec["source"] == "web":
            _capture_web(spec, args, raw)
        else:
            _capture_xephyr(spec, args, raw)
        _write_figures(raw, full_out, close_out, spec)
    return full_out, close_out


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--section",
        nargs="+",
        choices=("all", *SECTIONS),
        default=["all"],
        help="which control(s) to capture (default: all)",
    )
    parser.add_argument(
        "--out-dir",
        default=str(REPO / "docs" / "user" / "images"),
        help="where to write ui-<section>-window.png and ui-<section>.png",
    )
    parser.add_argument("--app-bin", default=str(DEFAULT_APP), help="the Parametric app binary")
    parser.add_argument(
        "--parent-display",
        default="",
        help="the X display Xephyr nests inside (default: $DISPLAY)",
    )
    parser.add_argument(
        "--window",
        type=shot.parse_size,
        default=list(DEFAULT_WINDOW),
        help=f"app window size (default: {DEFAULT_WINDOW[0]}x{DEFAULT_WINDOW[1]})",
    )
    parser.add_argument(
        "--screen",
        type=shot.parse_size,
        default=list(DEFAULT_SCREEN),
        help=f"Xephyr screen size (default: {DEFAULT_SCREEN[0]}x{DEFAULT_SCREEN[1]})",
    )
    parser.add_argument("--xephyr", default="Xephyr", help="the Xephyr executable")
    parser.add_argument("--chrome", default="", help="the Chrome/Chromium executable")
    parser.add_argument(
        "--web-dir",
        default=str(WEB_DIR),
        help="the built browser shell directory (default: crates/ferrocad_web_gpui/www)",
    )
    parser.add_argument("--port", type=int, default=0, help="web server port (default: pick one)")
    parser.add_argument("--wait", type=float, default=30.0, help="seconds to wait for the app window")
    args = parser.parse_args(argv)

    sections = list(SECTIONS) if "all" in args.section else args.section
    if any(SECTIONS[section]["source"] == "xephyr" for section in sections):
        if not (Path(args.xephyr).is_file() or shutil.which(args.xephyr)):
            print(f"error: {args.xephyr} is not installed", file=sys.stderr)
            return 1
        shot._require_x11_tools()
    if any(SECTIONS[section]["source"] == "web" for section in sections):
        try:
            _find_chrome(args.chrome)
        except RuntimeError as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1

    try:
        for section in sections:
            full, close = capture(section, args)
            print(f"wrote {full}")
            print(f"wrote {close}")
    except (RuntimeError, OSError, subprocess.TimeoutExpired) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
