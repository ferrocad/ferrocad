#!/usr/bin/env python3
"""Capture short animated GIFs of the desktop shell's pointer interactions.

The still figures in :mod:`ui_shots` show a control at rest. These GIFs show the
gesture: a tool switch, a Body/Face/Edge pick, an action-bar Create Box, a Move, a
pending Move and rollback, a Push/Pull drag, a Fillet scrub and a wheel flick. They
run the Parametric app in a private Xephyr server, synthesise the pointer with XTEST,
capture the window with ``xwd`` between events, and draw a synthetic cursor and a
caption on each frame.

A Web GIF is driven through the Chrome DevTools Protocol instead: one headless page
stays alive, the capture dispatches the same ``PointerEvent``s the page's own harness
uses, and it asks for a PNG between moves. Currently the shared Move gesture is the
Web section (``move-web``); the browser still lacks the Part push/pull and fillet
manipulators those other GIFs show. See ``docs/developer/workbenches/screenshot-tool.md``.

Usage::

    python3 tools/ui_gifs.py                       # every GIF
    python3 tools/ui_gifs.py --section pushpull fillet
    python3 tools/ui_gifs.py --section move move-web
    python3 tools/ui_gifs.py --section tools --keep-frames
    python3 tools/ui_gifs.py --out-dir docs/user/images

The output is ``ui-<section>.gif``. Frames are manual artefacts, like the
highlighted stills: they depend on the app, the X server and the font stack, so
they are not pinned by ``userdocs.py --check``.
"""

from __future__ import annotations

import argparse
import base64
import json
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
#: * ``source`` is ``"web"`` for a CDP-driven browser capture (default: Xephyr).
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

    "create": {
        "crop": (0, 840, 1100, 160),
        "scale": 0.7,
        "fps": 4,
        "steps": [
            {"caption": "Create Box adds a box", "click": (810, 943), "wait": 1.0, "duration": 1.2},
            {"caption": "Box001 is in the document", "wait": 1.0, "duration": 1.5},
        ],
    },
    "pending": {
        "crop": (0, 300, 1200, 700),
        "scale": 0.6,
        "fps": 5,
        "steps": [
            {"caption": "Move · drag an arrow", "click": (900, 450), "wait": 0.7, "duration": 0.9},
            {"caption": "Grab the X arrow", "move": (830, 515), "wait": 0.3, "duration": 0.5},
            {"press": (830, 515), "duration": 0.4},
            {"drag": [(845, 524), (860, 533), (875, 542)]},
            {"caption": "Pending: ✓ commits, Esc rolls back", "release": (875, 542), "wait": 0.7, "duration": 1.2},
            {"caption": "Esc rolls back", "key": "Escape", "wait": 0.7, "duration": 1.4},
        ],
    },
    "move": {
        "crop": (430, 140, 940, 780),
        "scale": 0.65,
        "fps": 6,
        "steps": [
            {"caption": "Move · select the filleted body", "click": (900, 450), "wait": 0.7, "duration": 0.9},
            {"caption": "Grab the X arrow", "move": (830, 515), "wait": 0.3, "duration": 0.5},
            {"press": (830, 515), "duration": 0.3},
            {"drag": [(845, 524), (860, 533), (875, 542)]},
            {"caption": "Release leaves it pending", "release": (875, 542), "wait": 0.7, "duration": 1.0},
            {"move": (822, 557), "wait": 0.3, "duration": 0.4},
            {"caption": "Click ✓ to accept", "click": (822, 557), "wait": 0.8, "duration": 1.4},
        ],
    },
    "move-web": {
        "source": "web",
        "crop": (120, 60, 1080, 900),
        "scale": 0.6,
        "fps": 6,
        "steps": [
            {"caption": "Move · select the filleted body", "click": (745, 410), "wait": 0.8, "duration": 0.9},
            {"caption": "Grab the X arrow", "move": (640, 452), "wait": 0.3, "duration": 0.5},
            {"press": (640, 452), "duration": 0.3},
            {"drag": [(655, 460), (670, 468), (685, 476), (690, 482)]},
            {"caption": "Release leaves it pending", "release": (690, 482), "wait": 0.7, "duration": 1.0},
            {"move": (628, 482), "wait": 0.3, "duration": 0.4},
            {"caption": "Click ✓ to accept", "click": (628, 482), "wait": 0.8, "duration": 1.4},
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


def _launch(
    display: str, args, settings: Path
) -> tuple[subprocess.Popen, str, int, int, tempfile.TemporaryDirectory]:
    """Start the app on ``display`` and return its process, window and origin.

    The returned workspace holds the throwaway parameter file (a copy of
    ``settings``) and empty mod set from
    :func:`ui_shots.desktop_capture_config`; the caller cleans it up after the
    app exits.
    """
    app = stills._app_binary(Path(args.app_bin))
    workspace, app_args, capture_env = stills.desktop_capture_config(app, settings)
    proc = None
    try:
        env = dict(os.environ)
        env["DISPLAY"] = display
        env["WAYLAND_DISPLAY"] = ""
        env["XDG_SESSION_TYPE"] = "x11"
        env.update(capture_env)
        proc = subprocess.Popen(
            app_args,
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
        return proc, wid, x, y, workspace
    except Exception:
        if proc is not None:
            proc.terminate()
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()
        workspace.cleanup()
        raise


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


def _web_pointer(action: str, x: int, y: int, buttons: int) -> str:
    """A PointerEvent on the page's canvas, the way the harness dispatches it."""
    return (
        "(() => { const c = document.querySelector('canvas'); if (!c) return false; "
        "c.dispatchEvent(new PointerEvent('%s', { pointerId: 1, pointerType: 'mouse', "
        "isPrimary: true, clientX: %d, clientY: %d, buttons: %d, "
        "bubbles: true, cancelable: true })); return true; })()"
        % (action, x, y, buttons)
    )


def capture_web(section: str, spec: dict, args, settings: Path) -> Path:
    """Record a Web gesture by driving the page through the Chrome DevTools Protocol.

    The still capture can use Chrome's one-shot `--screenshot`; a GIF needs frames
    between pointer moves, so this keeps one headless page alive and asks it for a
    PNG after each synthetic `PointerEvent`. The page's own harness dispatches the
    same events on its canvas (see `www/index.html`), so this is the browser's real
    input path, not a reimplementation of the shell.
    """
    try:
        import requests
        import websocket
    except ImportError as exc:  # pragma: no cover - environment dependent
        raise RuntimeError(
            "Web GIFs need `requests` and `websocket-client` "
            "(`pip install requests websocket-client`)"
        ) from exc

    web_dir = Path(args.web_dir)
    if not (web_dir / "pkg").is_dir():
        raise RuntimeError(
            f"{web_dir}/pkg is missing; build it with "
            "`crates/ferrocad_web_gpui/run.sh --build-only`"
        )
    chrome = stills._find_chrome(args.chrome)
    port = stills._free_port()
    served = web_dir / f"capture-gif-settings-{port}.json"
    shutil.copyfile(settings, served)
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
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    stills._wait_for_port(port)

    debug = stills._free_port()
    profile = tempfile.TemporaryDirectory(prefix="ferrocad-gif-chrome-")
    query = spec.get("query", "runtime=pywasm,js").lstrip("?")
    url = f"http://127.0.0.1:{port}/?{query}&settings_file={served.name}"
    chrome_proc = subprocess.Popen(
        [
            chrome,
            "--headless=new",
            "--no-sandbox",
            "--disable-dev-shm-usage",
            "--use-angle=swiftshader",
            "--enable-unsafe-swiftshader",
            "--window-size=1600,1000",
            f"--remote-debugging-port={debug}",
            f"--user-data-dir={profile.name}",
            "--no-first-run",
            "--no-default-browser-check",
            "--no-proxy-server",
            "about:blank",
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    stills._wait_for_port(debug)

    ws = None
    try:
        target = None
        for _ in range(100):
            for item in requests.get(
                f"http://127.0.0.1:{debug}/json", timeout=2
            ).json():
                if item.get("type") == "page":
                    target = item
                    break
            if target:
                break
            time.sleep(0.1)
        if target is None:
            raise RuntimeError("headless Chrome exposed no page target")
        ws = websocket.create_connection(
            target["webSocketDebuggerUrl"], timeout=30, suppress_origin=True
        )
        sequence = 0

        def cdp(method: str, **params):
            nonlocal sequence
            sequence += 1
            ws.send(json.dumps({"id": sequence, "method": method, "params": params}))
            while True:
                message = json.loads(ws.recv())
                if message.get("id") == sequence:
                    if "error" in message:
                        raise RuntimeError(message["error"])
                    return message.get("result", {})

        def evaluate(expression: str):
            answer = cdp(
                "Runtime.evaluate",
                expression=expression,
                returnByValue=True,
                awaitPromise=True,
            )
            return answer.get("result", {}).get("value")

        cdp("Page.enable")
        cdp("Runtime.enable")
        cdp("Page.navigate", url=url)
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            if evaluate("!!document.querySelector('canvas')"):
                break
            time.sleep(0.5)
        else:
            raise RuntimeError("the Web shell never drew its canvas")
        time.sleep(2.0)

        frames: list = []
        durations: list[int] = []
        pointer: tuple[int, int] | None = None
        pressed = False
        caption: str | None = None
        default = int(1000 / spec.get("fps", 6))

        with tempfile.TemporaryDirectory(prefix="ferrocad-gif-web-") as tmp:
            tmp_path = Path(tmp)
            frame_dir = Path(args.frames_dir) / section if args.keep_frames else None
            if frame_dir:
                frame_dir.mkdir(parents=True, exist_ok=True)

            def snap(hold: float | None = None) -> None:
                raw = tmp_path / f"frame-{len(frames):03d}.png"
                data = cdp("Page.captureScreenshot", format="png")["data"]
                raw.write_bytes(base64.b64decode(data))
                image = _render(raw, spec, pointer, pressed, caption)
                frames.append(image)
                durations.append(int(hold * 1000) if hold else default)
                if frame_dir:
                    image.save(frame_dir / f"{len(frames) - 1:03d}.png")

            if spec.get("intro", True):
                snap(hold=0.8)
            for step in spec["steps"]:
                if step.get("caption"):
                    caption = step["caption"]
                if "click" in step:
                    x, y = step["click"]
                    pointer = (x, y)
                    evaluate(_web_pointer("pointerdown", x, y, 1))
                    evaluate(_web_pointer("pointerup", x, y, 0))
                    time.sleep(step.get("wait", 0.5))
                elif "move" in step:
                    x, y = step["move"]
                    pointer = (x, y)
                    evaluate(_web_pointer("pointermove", x, y, 0))
                    time.sleep(step.get("wait", 0.3))
                elif "press" in step:
                    x, y = step["press"]
                    pointer = (x, y)
                    evaluate(_web_pointer("pointerdown", x, y, 1))
                    pressed = True
                    time.sleep(step.get("wait", 0.2))
                elif "drag" in step:
                    for x, y in step["drag"]:
                        pointer = (x, y)
                        evaluate(_web_pointer("pointermove", x, y, 1))
                        time.sleep(0.15)
                        snap(hold=0.12)
                    continue
                elif "release" in step:
                    x, y = step["release"]
                    pointer = (x, y)
                    evaluate(_web_pointer("pointerup", x, y, 0))
                    pressed = False
                    time.sleep(step.get("wait", 0.8))
                elif "wait" in step:
                    time.sleep(step["wait"])
                else:
                    raise RuntimeError(f"unknown Web step {step!r}")
                snap(hold=step.get("duration"))

        out = Path(args.out_dir) / f"ui-{section}.gif"
        _save(frames, durations, out)
        return out
    finally:
        if ws is not None:
            ws.close()
        chrome_proc.terminate()
        try:
            chrome_proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            chrome_proc.kill()
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()
        served.unlink(missing_ok=True)
        profile.cleanup()


def capture(section: str, spec: dict, args, display: str, settings: Path) -> Path:
    from Xlib import X

    proc, wid, x, y, workspace = _launch(display, args, settings)
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
            # After the app has stopped: it writes the parameter file on exit.
            workspace.cleanup()

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
    parser.add_argument("--chrome", default="", help="the Chrome/Chromium executable")
    parser.add_argument(
        "--web-dir",
        default=str(stills.WEB_DIR),
        help="the built browser shell directory",
    )
    parser.add_argument("--wait", type=float, default=20.0)
    parser.add_argument(
        "--settings",
        default="",
        help=(
            "whole settings file applied to the app (default: the desktop's "
            "~/.config/ferrocad/parameters.json, or a temporary seed)"
        ),
    )
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
    desktop_sections = [section for section in sections if GIFS[section].get("source") != "web"]
    web_sections = [section for section in sections if GIFS[section].get("source") == "web"]
    if desktop_sections:
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
    try:
        settings, settings_workspace = stills.resolve_capture_settings(
            args.settings or None
        )
    except RuntimeError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    display = xephyr = None
    try:
        if desktop_sections:
            display, xephyr = stills._start_xephyr(
                args.parent_display, args.screen, args.xephyr
            )
            for section in desktop_sections:
                out = capture(section, GIFS[section], args, display, settings)
                print(f"wrote {out}")
        for section in web_sections:
            out = capture_web(section, GIFS[section], args, settings)
            print(f"wrote {out}")
    finally:
        if xephyr is not None:
            xephyr.terminate()
            try:
                xephyr.wait(timeout=5)
            except subprocess.TimeoutExpired:
                xephyr.kill()
        if settings_workspace is not None:
            settings_workspace.cleanup()
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
