#!/usr/bin/env python3
"""S0 static audit of the SimpleCAD checkout.

Pins the numbers recorded in ``docs/workbenches/simplecad-bringup.md`` §2 and
compares the sibling checkout against ``contracts/simplecad.json``. The contract
is the source of truth; every value is counted from the checkout, never
hard-coded here. It is a drift alarm, not a gate on upstream content.

Metrics (all counted under ``<SIMPLECAD_DIR>``):

==========================  ==================================================
``py_files``                ``*.py`` files, excluding ``.git``/``__pycache__``
``part_names``              distinct ``Part.<name>`` spellings (``MeshPart`` excluded)
``ui_files``                ``*.ui`` files
``draft_imports``           lines with ``import Draft`` / ``from Draft``
``partdesign_files``        files with a ``PartDesign`` import
``viewprovider_refs``       lines mentioning ``ViewProvider``
``test_files``              every file under ``tests/``
==========================  ==================================================

Usage::

    python3 tools/simplecad_audit.py              # show the counted values
    python3 tools/simplecad_audit.py --check      # exit 1 on drift
    python3 tools/simplecad_audit.py --write      # re-pin the contract
    python3 tools/simplecad_audit.py --json       # machine-readable output

The checkout is ``--root``, then ``$SIMPLECAD_DIR`` / ``$FERROCAD_SIMPLECAD``,
then ``../simplecad`` beside the repo.

CI is optional at S0: the ``geometry`` job runs ``--check`` only when a sibling
``../simplecad`` checkout happens to exist, and normal PR CI never requires it.
A missing checkout exits 2, because the audit cannot verify anything without it.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]  # ferrocad project root
DEFAULT_ROOT = REPO.parent / "simplecad"
DEFAULT_CONTRACT = REPO / "contracts" / "simplecad.json"
UPSTREAM = "https://github.com/igsova/SimpleCAD.git"
MEASURED_FROM = "docs/workbenches/simplecad-bringup.md §2"

# Directories that are generated or vendored, never part of the measured tree.
IGNORED_DIRS = {".git", "__pycache__", ".venv"}

PART_NAME = re.compile(r"\bPart\.([A-Za-z_][A-Za-z0-9_]*)")
DRAFT_IMPORT = re.compile(r"\b(?:import\s+Draft\b|from\s+Draft\b)")
PARTDESIGN_IMPORT = re.compile(r"\b(?:import\s+PartDesign\b|from\s+PartDesign\b)")
VIEWPROVIDER = re.compile(r"\bViewProvider\b")
PACKAGE_VERSION = re.compile(r"<version>\s*([^<]+?)\s*</version>")

METRICS = (
    "py_files",
    "part_names",
    "ui_files",
    "draft_imports",
    "partdesign_files",
    "viewprovider_refs",
    "test_files",
)


def checkout_path(explicit: str | None = None) -> Path:
    """Resolve the checkout: CLI, then environment, then ``../simplecad``."""
    if explicit:
        return Path(explicit).expanduser().resolve()
    env = os.environ.get("SIMPLECAD_DIR") or os.environ.get("FERROCAD_SIMPLECAD")
    if env:
        return Path(env).expanduser().resolve()
    return DEFAULT_ROOT.resolve()


def _walk(root: Path):
    """Every file below ``root``, pruning generated/vendored directories."""
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in IGNORED_DIRS)
        for name in sorted(filenames):
            yield Path(dirpath) / name


def _matching_lines(text: str, pattern: re.Pattern[str]) -> int:
    return sum(1 for line in text.splitlines() if pattern.search(line))


def _revision(root: Path) -> str:
    try:
        out = subprocess.run(
            ["git", "--no-pager", "-C", str(root), "rev-parse", "--short", "HEAD"],
            capture_output=True,
            text=True,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return "unknown"
    return out.stdout.strip() or "unknown"


def _version(root: Path) -> str | None:
    try:
        manifest = (root / "package.xml").read_text(encoding="utf-8")
    except OSError:
        return None
    match = PACKAGE_VERSION.search(manifest)
    return match.group(1) if match else None


def count(root: Path) -> dict[str, int]:
    """Count the pinned metrics from the checkout."""
    files = list(_walk(root))
    py_files = [p for p in files if p.suffix == ".py"]

    part_names: set[str] = set()
    draft_imports = 0
    partdesign_files = 0
    viewprovider_refs = 0
    for path in py_files:
        text = path.read_text(encoding="utf-8", errors="replace")
        part_names.update(PART_NAME.findall(text))
        draft_imports += _matching_lines(text, DRAFT_IMPORT)
        partdesign_files += 1 if PARTDESIGN_IMPORT.search(text) else 0
        viewprovider_refs += _matching_lines(text, VIEWPROVIDER)

    tests = root / "tests"
    test_files = sum(1 for _ in _walk(tests)) if tests.is_dir() else 0

    return {
        "py_files": len(py_files),
        "part_names": len(part_names),
        "ui_files": sum(1 for p in files if p.suffix == ".ui"),
        "draft_imports": draft_imports,
        "partdesign_files": partdesign_files,
        "viewprovider_refs": viewprovider_refs,
        "test_files": test_files,
    }


def derive(root: Path) -> dict:
    """Build the contract payload from the checkout."""
    return {
        "schema": 1,
        "upstream": UPSTREAM,
        "measured_from": MEASURED_FROM,
        "revision": _revision(root),
        "version": _version(root),
        "counts": count(root),
    }


def load_pinned(path: Path) -> dict:
    if not path.is_file():
        raise SystemExit(
            f"simplecad audit: pinned contract not found: {path} "
            "(run with --write to create it)"
        )
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        raise SystemExit(f"simplecad audit: {path} is not valid JSON: {error}") from error


def compare(pinned: dict, found: dict) -> dict[str, dict[str, int | None]]:
    """Named per-metric deltas; empty means no drift."""
    expected = pinned.get("counts", {})
    observed = found.get("counts", {})
    drift: dict[str, dict[str, int | None]] = {}
    for key in sorted(set(expected) | set(observed)):
        want, got = expected.get(key), observed.get(key)
        if want != got:
            drift[key] = {"contract": want, "observed": got}
    return drift


def _delta(entry: dict[str, int | None]) -> str:
    want, got = entry["contract"], entry["observed"]
    text = f"contract {want!r}, observed {got!r}"
    if isinstance(want, int) and isinstance(got, int):
        text += f" (delta {got - want:+d})"
    return text


def summary(found: dict) -> str:
    counts = found["counts"]
    version = f", version {found['version']}" if found.get("version") else ""
    return (
        "simplecad audit: "
        f"{counts['py_files']} .py, {counts['part_names']} Part names, "
        f"{counts['ui_files']} .ui, {counts['draft_imports']} Draft imports, "
        f"{counts['partdesign_files']} PartDesign file, "
        f"{counts['viewprovider_refs']} ViewProvider, "
        f"{counts['test_files']} files under tests/ "
        f"({found['revision']}{version})"
    )


def _json_payload(
    root: Path,
    contract: Path,
    found: dict,
    drift: dict | None,
) -> dict:
    return {
        "root": str(root),
        "contract": str(contract),
        "revision": found["revision"],
        "version": found["version"],
        "counts": found["counts"],
        "drift": drift,
        "ok": not drift,
    }


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Count the SimpleCAD S0 metrics and compare them with the pinned "
            "contract. CI is optional at S0: the geometry job runs --check only "
            "when a sibling checkout exists, and normal PR CI never requires it."
        )
    )
    parser.add_argument(
        "--root",
        default=None,
        help=(
            "SimpleCAD checkout (default: $SIMPLECAD_DIR, $FERROCAD_SIMPLECAD "
            f"or {DEFAULT_ROOT})"
        ),
    )
    parser.add_argument(
        "--contract",
        default=str(DEFAULT_CONTRACT),
        help=f"pinned JSON file (default: {DEFAULT_CONTRACT})",
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="print a named delta and exit 1 on drift")
    mode.add_argument("--write", action="store_true", help="re-generate the pinned contract")
    parser.add_argument("--json", action="store_true", help="print machine-readable output")
    args = parser.parse_args(argv)

    root = checkout_path(args.root)
    contract_path = Path(args.contract)

    if not root.is_dir():
        message = (
            f"simplecad audit: checkout not found: {root} "
            "(set SIMPLECAD_DIR, FERROCAD_SIMPLECAD or --root)"
        )
        if args.json:
            print(
                json.dumps(
                    {
                        "root": str(root),
                        "contract": str(contract_path),
                        "error": "checkout not found",
                        "ok": False,
                    },
                    indent=2,
                )
            )
        else:
            print(message, file=sys.stderr)
        return 2

    found = derive(root)

    if args.write:
        contract_path.parent.mkdir(parents=True, exist_ok=True)
        contract_path.write_text(
            json.dumps(found, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
        )
        if args.json:
            print(json.dumps(found, indent=2, ensure_ascii=False))
        else:
            print(f"wrote {contract_path}")
            print(summary(found))
        return 0

    if args.check:
        pinned = load_pinned(contract_path)  # raises when the contract is missing
    else:
        pinned = load_pinned(contract_path) if contract_path.is_file() else None
    drift = compare(pinned, found) if pinned is not None else None

    if args.json:
        print(
            json.dumps(
                _json_payload(root, contract_path, found, drift),
                indent=2,
                ensure_ascii=False,
            )
        )
        return 1 if (args.check and drift) else 0

    if args.check:
        if drift:
            print(
                f"simplecad audit: {len(drift)} drift against "
                f"{contract_path} (declared upstream {pinned.get('revision')}):"
            )
            for key in sorted(drift):
                print(f"  {key}: {_delta(drift[key])}")
            print("Re-run `python3 tools/simplecad_audit.py --write` after confirming the change.")
            return 1
        print(f"{summary(found)} — no drift against {contract_path}")
        pinned_revision = pinned.get("revision")
        if pinned_revision and pinned_revision != found["revision"]:
            print(
                f"  note: revision differs: contract {pinned_revision}, "
                f"observed {found['revision']} (counts still match)"
            )
        return 0

    print(summary(found))
    for key in METRICS:
        print(f"  {key}: {found['counts'][key]}")
    print(f"  source: {root}")
    print(f"  contract: {contract_path} ({'present' if pinned else 'not written yet'})")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
