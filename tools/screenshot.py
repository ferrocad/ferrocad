#!/usr/bin/env python3
"""Capture a FerroCAD screenshot at one of three scopes.

Modes
-----

``viewport`` (default)
    Render a JSON spec through the offscreen renderer. This is the deterministic,
    GPU-only path used by ``tools/userdocs.py --capture``. It shows the 3D stage:
    no shell chrome and no desktop.

``chrome``
    Launch an edition, wait for its window and capture the window. This includes
    the shell chrome (title bar, header controls, action dock, Dev Tools if open)
    but not the surrounding desktop.

``desktop``
    Capture the whole display (or ``--region``). This includes the desktop and any
    other windows, in addition to the app.

The viewport mode works anywhere the renderer does. ``chrome`` and ``desktop`` use
X11 capture (``xwd`` plus a small built-in XWD reader) and need an X display; when
``DISPLAY`` is unset, ``chrome`` can start ``Xvfb`` if it is installed. A Wayland-only
desktop cannot be captured without a compositor/portal API, so the tool says so
instead of pretending.

Usage::

    python3 tools/screenshot.py --spec scene.json --out shot.png
    python3 tools/screenshot.py --workflow toy-workbench --out-dir target/shots
    python3 tools/screenshot.py --mode chrome --edition toy --out shots/toy.png
    python3 tools/screenshot.py --mode chrome --edition toy --size 1280x800 --out shots/toy.png
    python3 tools/screenshot.py --mode chrome --edition toy --dev-tools --out shots/shell.png
    python3 tools/screenshot.py --mode desktop --region 0,0,1920,1080 --out shots/desktop.png
    python3 tools/screenshot.py --list

Exit codes: ``0`` wrote the image; ``1`` a workflow/spec/I/O error; ``3`` no usable
GPU adapter in viewport mode. The command prints the reason in every case.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import time
import zlib
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
REPO = TOOLS.parent
sys.path.insert(0, str(TOOLS))

import userdocs  # noqa: E402

#: The edition binaries ``--mode chrome`` can launch.
EDITIONS = {
    "toy": ("ferrocad_toy", "ferrocad_toy"),
    "base": ("ferrocad", "ferrocad"),
    "parametric": ("ferrocad_parametric", "ferrocad_parametric"),
}

#: How long to wait for the app window (seconds).
DEFAULT_WAIT = 20.0

#: The default window/screen size for a display started by this tool.
DEFAULT_DISPLAY_SIZE = [1280, 800]


# ---------------------------------------------------------------------------
# Viewport mode
# ---------------------------------------------------------------------------
def parse_size(value: str) -> list[int]:
    """``1440x900`` → ``[1440, 900]``; the renderer rejects a zero dimension."""
    parts = value.lower().split("x")
    if len(parts) != 2:
        raise argparse.ArgumentTypeError("size must look like 1440x900")
    try:
        width, height = (int(part) for part in parts)
    except ValueError as exc:
        raise argparse.ArgumentTypeError("size must contain two integers") from exc
    if width <= 0 or height <= 0:
        raise argparse.ArgumentTypeError("size dimensions must be positive")
    return [width, height]


def parse_region(value: str) -> list[int]:
    """``x,y,w,h`` → four integers for ``--mode desktop``."""
    parts = value.split(",")
    if len(parts) != 4:
        raise argparse.ArgumentTypeError("region must look like x,y,width,height")
    try:
        values = [int(part) for part in parts]
    except ValueError as exc:
        raise argparse.ArgumentTypeError("region must contain four integers") from exc
    if values[2] <= 0 or values[3] <= 0:
        raise argparse.ArgumentTypeError("region width and height must be positive")
    return values


def load_spec(path: Path) -> dict:
    """Read a JSON snapshot spec (``-`` reads stdin)."""
    if str(path) == "-":
        text = sys.stdin.read()
        where = "stdin"
    else:
        try:
            text = path.read_text()
        except OSError as exc:
            raise RuntimeError(f"cannot read {path}: {exc}") from exc
        where = str(path)
    try:
        spec = json.loads(text)
    except json.JSONDecodeError as exc:
        raise RuntimeError(f"{where} is not valid JSON: {exc}") from exc
    if not isinstance(spec, dict):
        raise RuntimeError(f"{where} must contain a JSON object")
    return spec


def render_spec(spec: dict, out: Path, binary: Path) -> int:
    """Render one spec with the snapshot binary. Returns its exit code."""
    out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="ferrocad-screenshot-") as tmp:
        spec_path = Path(tmp) / "spec.json"
        spec_path.write_text(json.dumps(spec, indent=2, sort_keys=True) + "\n")
        proc = subprocess.run(
            [str(binary), str(spec_path), str(out)],
            cwd=REPO,
            capture_output=True,
            text=True,
        )
    if proc.stdout.strip():
        print(proc.stdout.strip())
    if proc.returncode == userdocs.SNAPSHOT_NO_ADAPTER:
        print(
            "error: no GPU adapter available; the image was not written",
            file=sys.stderr,
        )
        return 3
    if proc.returncode != 0:
        detail = proc.stderr.strip() or proc.stdout.strip() or "no output"
        print(f"error: snapshot render failed ({out}): {detail}", file=sys.stderr)
        return 1
    return 0


def workflow_path(slug: str) -> Path:
    """The workflow file declaring ``SLUG = slug``."""
    matches = []
    for path in userdocs.discover():
        text = path.read_text()
        if f'SLUG = "{slug}"' in text or f"SLUG = '{slug}'" in text:
            matches.append(path)
    if not matches:
        raise RuntimeError(f"no workflow declares SLUG = {slug!r}")
    if len(matches) > 1:
        names = ", ".join(path.name for path in matches)
        raise RuntimeError(f"slug {slug!r} is declared by more than one workflow: {names}")
    return matches[0]


def render_workflow(slug: str, out_dir: Path, binary: Path) -> int:
    """Render every ``image`` op with a spec in one workflow."""
    capture = userdocs.run_workflow(workflow_path(slug))
    images = [op for op in capture["ops"] if op["kind"] == "image"]
    if not images:
        print(f"{slug}: no image ops")
        return 0
    out_dir.mkdir(parents=True, exist_ok=True)
    written = 0
    for op in images:
        name = userdocs._safe_image_name(op["name"])
        if not op.get("spec"):
            print(f"{slug}: {name} has no render spec; skipped")
            continue
        code = render_spec(op["spec"], out_dir / name, binary)
        if code:
            return code
        written += 1
    print(f"{slug}: {written} image(s) in {out_dir}")
    return 0


def list_workflows() -> int:
    """Print each workflow and its generated image count."""
    for path in userdocs.discover():
        capture = userdocs.run_workflow(path)
        images = [
            op["name"]
            for op in capture["ops"]
            if op["kind"] == "image" and op.get("spec")
        ]
        suffix = f" — {', '.join(images)}" if images else ""
        print(f"{capture['slug']}{suffix}")
    return 0


# ---------------------------------------------------------------------------
# Display capture (chrome and desktop modes)
# ---------------------------------------------------------------------------
def _run(command: list[str], *, timeout: float = 30.0) -> subprocess.CompletedProcess:
    return subprocess.run(command, capture_output=True, text=True, timeout=timeout)


def _require_x11_tools() -> None:
    missing = [name for name in ("xwininfo", "xwd") if not shutil.which(name)]
    if missing:
        raise RuntimeError(
            "chrome/desktop capture needs the X11 tools "
            + ", ".join(missing)
            + " (install x11-utils/x11-apps, or use --mode viewport)"
        )


def _display_works(display: str) -> bool:
    try:
        return _run(["xwininfo", "-root", "-display", display], timeout=5).returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False


def _start_xvfb(size: list[int]) -> tuple[str, subprocess.Popen]:
    """Start a private Xvfb for headless chrome capture."""
    xvfb = shutil.which("Xvfb")
    if not xvfb:
        raise RuntimeError(
            "no DISPLAY and Xvfb is not installed; run under X11/Xvfb, "
            "or pass --display"
        )
    for number in range(99, 130):
        display = f":{number}"
        if os.path.exists(f"/tmp/.X11-unix/X{number}"):
            continue
        proc = subprocess.Popen(
            [
                xvfb,
                display,
                "-screen",
                f"{size[0]}x{size[1]}x24",
                "-ac",
                "-nolisten",
                "tcp",
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        deadline = time.monotonic() + 5.0
        while time.monotonic() < deadline:
            if proc.poll() is not None:
                break
            if _display_works(display):
                return display, proc
            time.sleep(0.1)
        proc.terminate()
        proc.wait(timeout=5)
    raise RuntimeError("could not start Xvfb on a free display")


def _ensure_display(args, *, allow_xvfb: bool) -> tuple[str, subprocess.Popen | None]:
    display = args.display or os.environ.get("DISPLAY")
    if display:
        if not _display_works(display):
            raise RuntimeError(f"cannot open display {display!r}")
        return display, None
    if allow_xvfb:
        size = args.size or DEFAULT_DISPLAY_SIZE
        return _start_xvfb(size)
    raise RuntimeError(
        "desktop capture needs an X display ($DISPLAY). A Wayland-only session "
        "has no portable root capture; use an Xvfb/X11 session or --display."
    )


def _window_tree(display: str) -> str:
    proc = _run(["xwininfo", "-root", "-tree", "-display", display])
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip() or f"cannot list windows on {display}")
    return proc.stdout


#: A line from `xwininfo -root -tree`: id, title, width, height, x, y.
_WINDOW_LINE = re.compile(
    r'^\s+(0x[0-9a-fA-F]+)\s+"([^"]*)":\s+\([^)]*\)\s+'
    r"(\d+)x(\d+)\+(-?\d+)\+(-?\d+)"
)


def _find_window(display: str, title: str) -> tuple[str, str, int, int, int, int] | None:
    matches = []
    for line in _window_tree(display).splitlines():
        match = _WINDOW_LINE.match(line)
        if not match:
            continue
        window_id, name, width, height, x, y = match.groups()
        if title.lower() in name.lower() and int(width) > 1 and int(height) > 1:
            matches.append((window_id, name, int(width), int(height), int(x), int(y)))
    if not matches:
        return None
    # The main window is the largest match; a floating Dev Tools window also
    # carries the application title.
    return max(matches, key=lambda window: window[2] * window[3])


def _wait_for_window(
    display: str, title: str, timeout: float, proc: subprocess.Popen, log: Path
) -> tuple[str, str, int, int, int, int]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if proc.poll() is not None:
            tail = log.read_text()[-3000:] if log.exists() else ""
            raise RuntimeError(
                f"the app exited before opening a window (exit {proc.returncode})"
                + (f"\n{tail}" if tail else "")
            )
        window = _find_window(display, title)
        if window:
            return window
        time.sleep(0.25)
    raise RuntimeError(
        f"no window matching {title!r} appeared within {timeout:g}s on {display}"
    )


def _resize_window(display: str, window_id: str, size: list[int]) -> None:
    """Best-effort resize; the app's default 1180×760 is used when Xlib is absent."""
    try:
        from Xlib import display as xdisplay
    except ImportError:
        print(
            "note: python-xlib is not installed; keeping the app's default window size",
            file=sys.stderr,
        )
        return
    try:
        screen = xdisplay.Display(display)
        window = screen.create_resource_object("window", int(window_id, 16))
        window.configure(width=size[0], height=size[1])
        screen.sync()
        screen.close()
    except Exception as exc:  # noqa: BLE001 - a resize hint is best effort
        print(f"note: could not resize the window: {exc}", file=sys.stderr)


def _app_binary(args) -> Path:
    if args.app_bin:
        path = Path(args.app_bin).expanduser()
        if not path.is_file():
            raise RuntimeError(f"--app-bin={args.app_bin} is not a file")
        return path
    package, binary = EDITIONS[args.edition]
    command = ["cargo", "build", "--offline", "-p", package]
    print("building the app: " + " ".join(command))
    proc = _run(command, timeout=600)
    if proc.returncode != 0:
        raise RuntimeError(
            f"could not build {package}\n--- stdout ---\n{proc.stdout}\n"
            f"--- stderr ---\n{proc.stderr}"
        )
    path = REPO / "target" / "debug" / binary
    if sys.platform == "win32":
        path = path.with_suffix(".exe")
    if not path.is_file():
        raise RuntimeError(f"cargo reported success but {path} does not exist")
    return path


def _xwd_to_rgba(data: bytes, region: list[int] | None) -> tuple[int, int, bytes]:
    """Decode a ZPixmap XWD (24/32 bpp TrueColor) to RGBA8.

    The XWD reader is deliberately small: Xvfb and Xwayland both give a 32-bpp
    TrueColor ZPixmap, which is the only shape this path needs. Anything else is
    reported; the caller can fall back to ImageMagick.
    """
    if len(data) < 100:
        raise RuntimeError("XWD file is shorter than its header")
    fields = struct.unpack(">25I", data[:100])
    (
        header_size,
        _version,
        pixmap_format,
        _depth,
        width,
        height,
        _xoffset,
        byte_order,
        _bitmap_unit,
        _bitmap_bit_order,
        _bitmap_pad,
        bits_per_pixel,
        bytes_per_line,
        visual_class,
        red_mask,
        green_mask,
        blue_mask,
        _bits_per_rgb,
        _colormap_entries,
        ncolors,
        _window_width,
        _window_height,
        _window_x,
        _window_y,
        _window_bdrwidth,
    ) = fields
    if pixmap_format != 2:
        raise RuntimeError("XWD is not a ZPixmap")
    if visual_class not in (4, 5):
        raise RuntimeError("XWD is not TrueColor/DirectColor")
    if bits_per_pixel not in (24, 32):
        raise RuntimeError(f"XWD has {bits_per_pixel} bits per pixel; expected 24 or 32")

    pixel_offset = header_size + ncolors * 12
    stride = 4 if bits_per_pixel == 32 else (4 if bytes_per_line // height >= width * 4 else 3)
    if bytes_per_line < width * stride:
        raise RuntimeError("XWD row is shorter than its width")
    if len(data) < pixel_offset + bytes_per_line * height:
        raise RuntimeError("XWD pixel data is truncated")

    def channel(word: int, mask: int) -> int:
        if not mask:
            return 255
        shift = (mask & -mask).bit_length() - 1
        maximum = mask >> shift
        return ((word & mask) >> shift) * 255 // maximum

    endian = "little" if byte_order == 0 else "big"
    rgba = bytearray(width * height * 4)
    for y in range(height):
        row = data[pixel_offset + y * bytes_per_line : pixel_offset + (y + 1) * bytes_per_line]
        for x in range(width):
            start = x * stride
            word = int.from_bytes(row[start : start + stride], endian)
            target = (y * width + x) * 4
            rgba[target] = channel(word, red_mask)
            rgba[target + 1] = channel(word, green_mask)
            rgba[target + 2] = channel(word, blue_mask)
            rgba[target + 3] = 255

    if region:
        x0, y0, region_width, region_height = region
        if x0 < 0 or y0 < 0 or x0 + region_width > width or y0 + region_height > height:
            raise RuntimeError(
                f"region {region} is outside the {width}×{height} capture"
            )
        cropped = bytearray(region_width * region_height * 4)
        for row in range(region_height):
            source = ((y0 + row) * width + x0) * 4
            cropped[row * region_width * 4 : (row + 1) * region_width * 4] = rgba[
                source : source + region_width * 4
            ]
        return region_width, region_height, bytes(cropped)
    return width, height, bytes(rgba)


def _write_png(path: Path, width: int, height: int, rgba: bytes) -> None:
    """Write RGBA8 PNG deterministically (no timestamp, fixed compression)."""
    if len(rgba) != width * height * 4:
        raise RuntimeError("RGBA buffer does not match the image size")

    def chunk(kind: bytes, payload: bytes) -> bytes:
        return (
            struct.pack(">I", len(payload))
            + kind
            + payload
            + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
        )

    scanlines = bytearray()
    stride = width * 4
    for row in range(height):
        scanlines.append(0)
        scanlines.extend(rgba[row * stride : (row + 1) * stride])

    png = bytearray(b"\x89PNG\r\n\x1a\n")
    png.extend(chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)))
    png.extend(chunk(b"IDAT", zlib.compress(bytes(scanlines), 9)))
    png.extend(chunk(b"IEND", b""))
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.tmp")
    temporary.write_bytes(png)
    os.replace(temporary, path)


def _xwd_to_png(xwd: Path, out: Path, region: list[int] | None) -> None:
    """Convert one XWD file to PNG, falling back to ImageMagick if needed."""
    try:
        width, height, rgba = _xwd_to_rgba(xwd.read_bytes(), region)
        _write_png(out, width, height, rgba)
        return
    except RuntimeError as exc:
        if not shutil.which("convert"):
            raise RuntimeError(
                f"cannot decode {xwd}: {exc} (install ImageMagick for the fallback)"
            ) from exc
        command = ["convert", str(xwd)]
        if region:
            x, y, w, h = region
            command += ["-crop", f"{w}x{h}+{x}+{y}", "+repage"]
        command.append(str(out))
        proc = _run(command, timeout=60)
        if proc.returncode != 0:
            raise RuntimeError(
                f"cannot decode {xwd}: {exc}; ImageMagick also failed: "
                f"{proc.stderr.strip()}"
            ) from exc


def _capture_window(display: str, window_id: str, out: Path, region: list[int] | None) -> None:
    with tempfile.TemporaryDirectory(prefix="ferrocad-xwd-") as tmp:
        xwd = Path(tmp) / "window.xwd"
        proc = _run(["xwd", "-display", display, "-id", window_id, "-out", str(xwd)])
        if proc.returncode != 0:
            raise RuntimeError(proc.stderr.strip() or f"xwd failed for {window_id}")
        _xwd_to_png(xwd, out, region)


def _capture_root(display: str, out: Path, region: list[int] | None) -> None:
    with tempfile.TemporaryDirectory(prefix="ferrocad-xwd-") as tmp:
        xwd = Path(tmp) / "root.xwd"
        proc = _run(["xwd", "-display", display, "-root", "-out", str(xwd)])
        if proc.returncode != 0:
            raise RuntimeError(
                (proc.stderr.strip() or "xwd could not capture the root window")
                + " (a Wayland desktop has no portable root capture; use an X11/Xvfb "
                "session or capture the app window with --mode chrome)"
            )
        _xwd_to_png(xwd, out, region)


def _copy_atomic(source: Path, out: Path) -> None:
    out.parent.mkdir(parents=True, exist_ok=True)
    temporary = out.with_name(f".{out.name}.tmp")
    temporary.write_bytes(source.read_bytes())
    os.replace(temporary, out)


def _crop_png(path: Path, region: list[int]) -> None:
    """Crop a PNG in place, for a portal capture that has no region option."""
    try:
        from PIL import Image
    except ImportError as exc:
        raise RuntimeError(
            "cropping a portal desktop capture needs Pillow (`pip install pillow`)"
        ) from exc
    x, y, width, height = region
    with Image.open(path) as image:
        if x < 0 or y < 0 or x + width > image.width or y + height > image.height:
            raise RuntimeError(
                f"region {region} is outside the {image.width}×{image.height} capture"
            )
        image.crop((x, y, x + width, y + height)).save(path)


def _grim_screenshot(out: Path, region: list[int] | None) -> bool:
    grim = shutil.which("grim")
    if not grim:
        return False
    out.parent.mkdir(parents=True, exist_ok=True)
    command = [grim]
    if region:
        x, y, width, height = region
        command += ["-g", f"{x},{y} {width}x{height}"]
    command.append(str(out))
    proc = _run(command, timeout=60)
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip() or "grim failed to capture the desktop")
    return True


def _portal_screenshot(out: Path, region: list[int] | None, timeout: float = 90.0) -> None:
    """Capture the live Wayland desktop through xdg-desktop-portal.

    This is the only portable Wayland root capture. It may show a permission
    prompt; the timeout is deliberately generous, and the failure says so.
    """
    try:
        import gi

        gi.require_version("GLib", "2.0")
        from gi.repository import Gio, GLib
    except (ImportError, ValueError) as exc:
        raise RuntimeError(
            "Wayland desktop capture needs grim or PyGObject (xdg-desktop-portal)"
        ) from exc

    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    token = f"ferrocad_{os.getpid()}_{time.time_ns()}"
    unique = bus.get_unique_name().lstrip(":").replace(".", "_")
    request_path = f"/org/freedesktop/portal/desktop/request/{unique}/{token}"
    loop = GLib.MainLoop()
    result: dict[str, object] = {}

    def on_response(_connection, _sender, _path, _interface, _signal, params, _user_data=None):
        response, values = params.unpack()
        if response == 0:
            result["uri"] = values.get("uri")
        else:
            result["error"] = response
        loop.quit()

    subscription = bus.signal_subscribe(
        "org.freedesktop.portal.Desktop",
        "org.freedesktop.portal.Request",
        "Response",
        request_path,
        None,
        Gio.DBusSignalFlags.NONE,
        on_response,
    )
    GLib.timeout_add_seconds(int(timeout), lambda: (loop.quit(), False)[1])
    try:
        bus.call_sync(
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Screenshot",
            "Screenshot",
            GLib.Variant(
                "(sa{sv})",
                (
                    "",
                    {
                        "handle_token": GLib.Variant("s", token),
                        "interactive": GLib.Variant("b", False),
                    },
                ),
            ),
            GLib.VariantType.new("(o)"),
            Gio.DBusCallFlags.NONE,
            5000,
            None,
        )
        loop.run()
    except GLib.Error as exc:
        raise RuntimeError(f"the desktop screenshot portal failed: {exc}") from exc
    finally:
        bus.signal_unsubscribe(subscription)

    uri = result.get("uri")
    if not uri:
        if "error" in result:
            raise RuntimeError(
                "the desktop screenshot portal was dismissed or denied"
            )
        raise RuntimeError(
            "the desktop screenshot portal did not answer (approve the prompt "
            "and try again, or use --mode chrome)"
        )
    source = Path(GLib.filename_from_uri(uri)[0])
    _copy_atomic(source, out)
    if region:
        _crop_png(out, region)


def _send_dev_tools(display: str, window_id: str) -> None:
    """Focus the main window and send Ctrl+Shift+D (Dev_ToggleTools)."""
    try:
        from Xlib import X, XK, display as xdisplay
        from Xlib.ext import xtest
    except ImportError as exc:
        raise RuntimeError("--dev-tools needs python-xlib") from exc
    screen = xdisplay.Display(display)
    try:
        window = screen.create_resource_object("window", int(window_id, 16))
        window.set_input_focus(X.RevertToParent, X.CurrentTime)
        screen.sync()
        sequence = [
            (X.KeyPress, "Control_L"),
            (X.KeyPress, "Shift_L"),
            (X.KeyPress, "d"),
            (X.KeyRelease, "d"),
            (X.KeyRelease, "Shift_L"),
            (X.KeyRelease, "Control_L"),
        ]
        for kind, name in sequence:
            keycode = screen.keysym_to_keycode(XK.string_to_keysym(name))
            xtest.fake_input(screen, kind, keycode)
            screen.sync()
    finally:
        screen.close()


def _compose_side_by_side(left: Path, right: Path, out: Path, gap: int = 12) -> None:
    """Compose two window captures, for a chrome screenshot that includes Dev Tools."""
    try:
        from PIL import Image
    except ImportError as exc:
        raise RuntimeError("--dev-tools needs Pillow (`pip install pillow`)") from exc
    with Image.open(left) as first, Image.open(right) as second:
        first = first.convert("RGB")
        second = second.convert("RGB")
        height = max(first.height, second.height)
        canvas = Image.new(
            "RGB", (first.width + gap + second.width, height), (15, 18, 24)
        )
        canvas.paste(first, (0, 0))
        canvas.paste(second, (first.width + gap, 0))
        canvas.save(out)


def _wayland_desktop(out: Path, region: list[int] | None) -> None:
    """grim, then the portal; both capture the live Wayland desktop."""
    if _grim_screenshot(out, region):
        return
    _portal_screenshot(out, region)


def capture_chrome(args) -> int:
    _require_x11_tools()
    display, xvfb = _ensure_display(args, allow_xvfb=True)
    app = _app_binary(args)
    out = Path(args.out)
    log_fd, log_name = tempfile.mkstemp(prefix="ferrocad-app-", suffix=".log")
    log = Path(log_name)
    log_file = os.fdopen(log_fd, "w")
    environment = dict(os.environ)
    environment["DISPLAY"] = display
    environment["WAYLAND_DISPLAY"] = ""
    environment["XDG_SESSION_TYPE"] = "x11"
    command = [str(app)]
    if args.open:
        command += ["--open", str(args.open)]
    proc = subprocess.Popen(
        command, cwd=REPO, env=environment, stdout=log_file, stderr=subprocess.STDOUT
    )
    try:
        window = _wait_for_window(display, args.title, args.wait, proc, log)
        window_id, name, *_geometry = window
        # Let the first frame land before resizing; a GPU surface can be blank
        # until the app has drawn once.
        time.sleep(1.0)
        if args.size:
            _resize_window(display, window_id, args.size)
            time.sleep(2.0)
        # One more poll after a resize so the window geometry is the final one.
        window = _find_window(display, name) or window
        if not args.dev_tools:
            _capture_window(display, window[0], out, None)
            print(f"wrote {out} ({window[2]}×{window[3]} from {name!r} on {display})")
            return 0

        with tempfile.TemporaryDirectory(prefix="ferrocad-chrome-") as tmp:
            main_png = Path(tmp) / "main.png"
            dev_png = Path(tmp) / "dev-tools.png"
            # Capture the main window *before* Dev Tools opens: on a bare X server
            # (Xvfb/Xephyr, no compositor) an overlapping window is part of the
            # pixels xwd reads back, so a later capture would contain both.
            _capture_window(display, window[0], main_png, None)
            _send_dev_tools(display, window[0])
            dev = _wait_for_window(display, "Dev Tools", min(args.wait, 10), proc, log)
            # The window exists before it has painted; the same first-frame delay
            # the main window needs applies here.
            time.sleep(1.0)
            _capture_window(display, dev[0], dev_png, None)
            _compose_side_by_side(main_png, dev_png, out)
        print(
            f"wrote {out} (main {window[2]}×{window[3]} + Dev Tools "
            f"{dev[2]}×{dev[3]} on {display})"
        )
        return 0
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait(timeout=5)
        log_file.close()
        log.unlink(missing_ok=True)
        if xvfb:
            xvfb.terminate()
            try:
                xvfb.wait(timeout=5)
            except subprocess.TimeoutExpired:
                xvfb.kill()


def capture_desktop(args) -> int:
    out = Path(args.out)
    display = args.display or os.environ.get("DISPLAY")
    if display and _display_works(display):
        try:
            _require_x11_tools()
            _capture_root(display, out, args.region)
            print(f"wrote {out} (desktop {display})")
            return 0
        except RuntimeError as exc:
            if args.display:
                raise
            wayland = bool(os.environ.get("WAYLAND_DISPLAY")) or (
                os.environ.get("XDG_SESSION_TYPE", "").lower() == "wayland"
            )
            if not wayland:
                raise
            # Xwayland cannot read its root window. On a Wayland session, fall
            # through to grim/the portal instead of failing the whole command.
            print(f"note: X11 root capture failed ({exc}); trying Wayland", file=sys.stderr)
    wayland = bool(os.environ.get("WAYLAND_DISPLAY")) or (
        os.environ.get("XDG_SESSION_TYPE", "").lower() == "wayland"
    )
    if not wayland:
        raise RuntimeError(
            "desktop capture needs an X display or a Wayland session "
            "($DISPLAY / $WAYLAND_DISPLAY); use --mode chrome for the app window"
        )
    _wayland_desktop(out, args.region)
    print(f"wrote {out} (Wayland desktop)")
    return 0


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------
def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--mode",
        choices=("viewport", "chrome", "desktop"),
        default="viewport",
        help="what to capture (default: viewport)",
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--spec", metavar="PATH", help="viewport: render one JSON spec ('-' = stdin)")
    mode.add_argument("--workflow", metavar="SLUG", help="viewport: render a workflow's image ops")
    mode.add_argument("--list", action="store_true", help="viewport: list workflows and their images")
    parser.add_argument("--out", metavar="PATH", help="output PNG (spec/chrome/desktop)")
    parser.add_argument(
        "--out-dir",
        metavar="DIR",
        default="target/screenshots",
        help="viewport --workflow output directory (default: target/screenshots)",
    )
    parser.add_argument(
        "--size",
        metavar="WxH",
        type=parse_size,
        help="viewport spec override, or the window size for chrome",
    )
    parser.add_argument(
        "--region",
        metavar="X,Y,W,H",
        type=parse_region,
        help="desktop capture region (default: the whole display)",
    )
    parser.add_argument(
        "--edition",
        choices=tuple(EDITIONS),
        default="toy",
        help="chrome: which edition to launch (default: toy)",
    )
    parser.add_argument("--app-bin", metavar="PATH", help="chrome: use this app binary")
    parser.add_argument("--open", metavar="FILE", help="chrome: open this document at boot")
    parser.add_argument(
        "--title",
        metavar="TEXT",
        default="FerroCAD",
        help="chrome: window-title substring to wait for (default: FerroCAD)",
    )
    parser.add_argument(
        "--wait",
        metavar="SECONDS",
        type=float,
        default=DEFAULT_WAIT,
        help=f"chrome: how long to wait for the window (default: {DEFAULT_WAIT:g})",
    )
    parser.add_argument(
        "--dev-tools",
        action="store_true",
        help=(
            "chrome: open Dev Tools and compose both windows side by side "
            "(needs python-xlib and Pillow)"
        ),
    )
    parser.add_argument(
        "--display",
        metavar="DISPLAY",
        help="X display for chrome/desktop (default: $DISPLAY; chrome starts Xvfb if unset)",
    )
    parser.add_argument(
        "--snapshot-bin",
        metavar="PATH",
        help="viewport: use this renderer instead of building ferrocad_snapshot",
    )
    args = parser.parse_args(argv)

    if args.list:
        if args.mode != "viewport":
            parser.error("--list is only for --mode viewport")
        return list_workflows()

    if args.dev_tools and args.mode != "chrome":
        parser.error("--dev-tools is only for --mode chrome")

    if args.mode == "viewport":
        if args.spec and not args.out:
            parser.error("--spec needs --out")
        if args.out == "-":
            parser.error("--out - is not supported; use ferrocad_snapshot for stdout")
        if args.workflow and args.out:
            parser.error("--out is for --spec; --workflow writes into --out-dir")
        if args.size and not args.spec:
            parser.error("--size is only meaningful with --spec")
        if not (args.spec or args.workflow):
            parser.error("choose --spec, --workflow or --list")
        try:
            binary = userdocs.snapshot_binary(args.snapshot_bin)
            if args.spec:
                spec = load_spec(Path(args.spec))
                if args.size:
                    spec["size"] = args.size
                return render_spec(spec, Path(args.out), binary)
            return render_workflow(args.workflow, Path(args.out_dir), binary)
        except (RuntimeError, OSError) as exc:
            print(f"error: {exc}", file=sys.stderr)
            return 1

    if not args.out:
        parser.error(f"--mode {args.mode} needs --out")
    if args.out == "-":
        parser.error("--out - is not supported; use ferrocad_snapshot for stdout")
    if args.spec or args.workflow:
        parser.error("--spec/--workflow are viewport-only; drop them for chrome/desktop")
    if args.mode == "desktop" and args.size:
        parser.error("--size is for viewport/chrome; desktop uses the display size")
    if args.mode == "chrome" and args.region:
        parser.error("--region is for desktop; chrome captures the whole window")
    if args.display and not _display_works(args.display):
        print(f"error: cannot open display {args.display!r}", file=sys.stderr)
        return 1
    try:
        if args.mode == "chrome":
            return capture_chrome(args)
        return capture_desktop(args)
    except (RuntimeError, OSError, subprocess.TimeoutExpired) as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
