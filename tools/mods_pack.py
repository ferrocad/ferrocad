#!/usr/bin/env python3
"""Build the ``mods/`` workbenches into packs the browser can fetch.

The page has no filesystem, so ``ferrocad_shell.load_workbenches`` cannot scan a
``mods/`` tree there — but it can ``fetch``. This tool turns each workbench in
``mods/`` into one JSON pack and writes an index beside them::

    mods/<Name>/            ->  crates/ferrocad_web_gpui/www/mods/<Name>.json
    (the set of them)       ->  crates/ferrocad_web_gpui/www/mods/index.json

A pack is::

    {"name": "<Name>", "version": 1,
     "files": {"Init.py": "<source>", "InitGui.py": "<source>",
               "Resources/<file>": "<text>"}}

and the index is::

    {"version": 1, "mods": [{"name": "<Name>", "pack": "<Name>.json"}, ...]}

The browser host hands each fetched pack to
``ferrocad_shell_core::app::load_workbench_pack``, whose Python half flattens it to the
``[{name, script, source}]`` entries ``ferrocad_shell.load_workbench_sources`` runs,
``Init.py`` then ``InitGui.py`` per mod in index order — the same order and the same
exec rules the filesystem loader uses, so a tree and a pack cannot report different
things about the same script (``docs/project/tasks/backend-parity.md`` B19).

**The entry scripts and ``Resources/`` travel; nothing else does.** A mod that
ships a shader or a template reads it with ``open(os.path.join(os.path.dirname(
os.path.abspath(__file__)), "Resources", ...))`` — the path a ``mods/`` tree gives it
— so the pack carries every regular file below ``Resources/`` under that same relative
key, and the host installs it where the loader's ``__file__`` points. A workbench that
imports a sibling module is still a later slice (it needs an import hook, not a file),
and binary assets such as icons ship later as separate URLs named by the pack rather
than base64.

JSON, not a zip, because ``fetch`` + ``JSON.parse`` need no unzip crate in the
wasm module. The pack is a build artifact *of the directory the desktop reads*,
which is why ``--check`` exists: a mod edited in ``mods/`` and not re-packed is
caught instead of shipping a stale script.

What makes the output deterministic, so the check compares bytes:

* ``sort_keys`` and a fixed indent, UTF-8, ``ensure_ascii=False``;
* sources are read in text mode, so ``\\r\\n`` becomes ``\\n`` on any checkout;
* no timestamps, no absolute paths, no interpreter version;
* workbenches and index entries in name order.

Usage::

    python3 tools/mods_pack.py            # write www/mods/
    python3 tools/mods_pack.py --check    # exit 1 if what is on disk is not what mods/ produces
    python3 tools/mods_pack.py --list     # the packs this would write, and their files

``--check`` also reports a pack left behind by a workbench that was removed: the
index must not advertise a mod the tree no longer has, and a stale
``<Name>.json`` would still be fetchable. Exits 2 when ``mods/`` is missing,
because a packer that silently packed nothing is worse than none.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
MODS = REPO / "mods"
OUT = REPO / "crates" / "ferrocad_web_gpui" / "www" / "mods"

#: A workbench's entry scripts, in the order the loader runs them.
SCRIPTS = ("Init.py", "InitGui.py")

#: The directory whose contents a mod reads with `open(...)` at run time.
RESOURCES = "Resources"

#: The pack format's version, carried by every pack and by the index.
VERSION = 1


# ---------------------------------------------------------------------------
# Sources
# ---------------------------------------------------------------------------
def entry_scripts(directory: Path) -> list[str]:
    """The entry scripts `directory` declares, in load order.

    The same rule `ferrocad_shell_python::mods` and the Python loader apply: a
    workbench is a directory holding at least one of :data:`SCRIPTS` at its top
    level. Files below it belong to the workbench but are not entries.
    """
    return [script for script in SCRIPTS if (directory / script).is_file()]


def workbenches(root: Path) -> list[tuple[str, Path]]:
    """`root`'s workbench directories, as ``(name, path)`` in name order.

    Directories that declare no entry script are skipped: ``mods/`` also holds
    ``README.md`` and the JS/wasm-only packages (`Explode`, `Fasteners`,
    `Spring`), which the Python loader would not run either.
    """
    found = []
    for child in sorted(root.iterdir(), key=lambda path: path.name):
        if child.is_dir() and entry_scripts(child):
            found.append((child.name, child))
    return found


def _encode(data: dict) -> str:
    """`data` as the bytes-stable JSON text every generated file is."""
    return json.dumps(data, sort_keys=True, ensure_ascii=False, indent=2) + "\n"


def resource_files(directory: Path) -> list[str]:
    """The workbench's ``Resources/`` files, as ``/``-separated relative paths.

    Name order, so the pack is bytes-stable. Only regular files: a directory is not
    a resource, and a link is not followed (nothing in ``mods/`` uses one).
    """
    root = directory / RESOURCES
    if not root.is_dir():
        return []
    return sorted(
        path.relative_to(directory).as_posix()
        for path in root.rglob("*")
        if path.is_file()
    )


def render_pack(name: str, directory: Path) -> str:
    """One workbench as its pack text.

    `Path.read_text` reads in text mode, so a CRLF checkout and an LF one produce
    the same pack; the JSON encoder escapes the newlines inside those strings.
    """
    files = {}
    for script in entry_scripts(directory):
        files[script] = (directory / script).read_text(encoding="utf-8")
    for resource in resource_files(directory):
        files[resource] = (directory / resource).read_text(encoding="utf-8")
    return _encode({"name": name, "version": VERSION, "files": files})


def render_index(names: list[str]) -> str:
    """The index over `names`, which must already be in name order."""
    return _encode(
        {
            "version": VERSION,
            "mods": [{"name": name, "pack": f"{name}.json"} for name in names],
        }
    )


def build(root: Path = MODS) -> tuple[dict[Path, str], list[tuple[str, list[str]]]]:
    """The files to write, and ``(name, scripts)`` per pack for reporting.

    The index is last in both, so a write reports the packs then the index, and
    the pack dict is keyed by the file it goes to.
    """
    outputs: dict[Path, str] = {}
    summary: list[tuple[str, list[str]]] = []
    names = []
    for name, directory in workbenches(root):
        names.append(name)
        summary.append((name, entry_scripts(directory) + resource_files(directory)))
        outputs[OUT / f"{name}.json"] = render_pack(name, directory)
    outputs[OUT / "index.json"] = render_index(names)
    return outputs, summary


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------
def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit 1 if the packs on disk are not what mods/ produces (writes nothing)",
    )
    parser.add_argument(
        "--list", action="store_true", help="print the packs and their scripts, then exit"
    )
    args = parser.parse_args(argv)

    if not MODS.is_dir():
        print(f"mods pack: no {MODS.relative_to(REPO)}/ directory to pack", file=sys.stderr)
        return 2

    outputs, summary = build()
    if not summary:
        print(
            f"mods pack: {MODS.relative_to(REPO)}/ holds no workbench "
            "(a directory with Init.py or InitGui.py)",
            file=sys.stderr,
        )
        return 2

    if args.list:
        for name, scripts in summary:
            print(f"{name}: {' '.join(scripts)}")
        return 0

    # A pack for a workbench that is gone is drift in the other direction: the
    # index would not name it, but the file would still be fetchable.
    generated = set(outputs)
    orphans = (
        sorted(path for path in OUT.glob("*.json") if path.is_file() and path not in generated)
        if OUT.is_dir()
        else []
    )

    stale: list[Path] = []
    for target, text in outputs.items():
        current = target.read_bytes() if target.is_file() else None
        if current == text.encode("utf-8"):
            continue
        stale.append(target)
        if args.check:
            continue
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text, encoding="utf-8", newline="\n")
        print(f"wrote {target.relative_to(REPO)}")

    if args.check:
        for target in stale + orphans:
            print(f"stale: {target.relative_to(REPO)}", file=sys.stderr)
        if stale or orphans:
            print(
                f"error: {len(stale)} pack(s) out of date, {len(orphans)} left behind; "
                "run `python3 tools/mods_pack.py`",
                file=sys.stderr,
            )
            return 1
        print(
            f"mod packs are current ({len(summary)} packs, "
            f"{sum(len(scripts) for _, scripts in summary)} scripts)"
        )
        return 0

    for target in orphans:
        target.unlink()
        print(f"removed {target.relative_to(REPO)}")

    print(f"packed {len(summary)} workbenches into {OUT.relative_to(REPO)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
