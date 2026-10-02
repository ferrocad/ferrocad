#!/usr/bin/env python3
"""M3d conformance harness.

Runs a curated set of upstream headless test files against **our** FreeCAD
implementation and reports, per test, whether it passed, failed, errored, or
could not be loaded (and why). The point is to make the API-parity gap
*measurable*: each "load error" or "error" names a missing piece of surface
(e.g. ``FreeCAD.Base``, ``obj.ID``, ``doc.saveAs``), which feeds the M4 wiring
work.

Usage::

    python3 tools/conformance.py --root ../freecad-upstream            # default files
    python3 tools/conformance.py --root ../freecad-upstream --files Document StringHasher
    python3 tools/conformance.py --root ../freecad-upstream --list     # list candidates
"""

from __future__ import annotations

import argparse
import importlib.util
import re
import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]   # freecad-rs-poc
PYTHON_DIR = REPO / "python"

DEFAULT_FILES = ["Document", "StringHasher", "UnitTests", "UnicodeTests", "TestApp"]


def _short(err) -> str:
    """Turn a unittest ``err`` tuple into a one-line, frame-tagged message."""
    exc_type, exc_value, tb = err
    last = None
    while tb is not None:
        last = (tb.tb_frame.f_code.co_filename, tb.tb_lineno)
        tb = tb.tb_next
    text = str(exc_value).strip()
    msg = text.splitlines()[0] if text else exc_type.__name__
    loc = f"{Path(last[0]).name}:{last[1]}" if last else ""
    return f"{exc_type.__name__}: {msg}  ({loc})"[:300]


class Collector(unittest.TestResult):
    """Collects per-test outcomes instead of printing them."""

    def __init__(self):
        super().__init__()
        self.records: list[tuple[str, str, str]] = []

    def addSuccess(self, test):
        self.records.append((test.id(), "pass", ""))

    def addFailure(self, test, err):
        self.records.append((test.id(), "fail", _short(err)))

    def addError(self, test, err):
        self.records.append((test.id(), "error", _short(err)))

    def addSkip(self, test, reason):
        self.records.append((test.id(), "skip", reason))


def load_module(path: Path):
    """Import a test file as a module; return (module, None) or (None, error)."""
    spec = importlib.util.spec_from_file_location(path.stem, path)
    module = importlib.util.module_from_spec(spec)
    try:
        spec.loader.exec_module(module)
    except Exception as exc:  # noqa: BLE001 - we want any import failure
        return None, exc
    return module, None


def classify_load_error(exc: Exception) -> str:
    msg = str(exc)
    # "cannot import name 'X' from 'Y'"  ->  "missing 'Y.X'"
    if "cannot import name" in msg:
        m = re.search(r"cannot import name '([^']+)' from '([^']+)'", msg)
        if m:
            return f"missing '{m.group(2)}.{m.group(1)}'"
    name = getattr(exc, "name", None)
    if name is not None:
        return f"missing module '{name}'"
    return f"{type(exc).__name__}: {exc}"


def run_file(path: Path):
    """Load and run one test file; return a dict describing the outcome."""
    module, err = load_module(path)
    if module is None:
        return {"file": path.name, "loaded": False, "reason": classify_load_error(err)}

    suite = unittest.defaultTestLoader.loadTestsFromModule(module)
    result = Collector()
    suite.run(result)
    return {
        "file": path.name,
        "loaded": True,
        "tests": result.records,
        "totals": _totals(result.records),
    }


def _totals(records) -> dict[str, int]:
    out = {"pass": 0, "fail": 0, "error": 0, "skip": 0}
    for _, status, _ in records:
        out[status] += 1
    return out


def list_candidates(root: Path) -> list[str]:
    test_dir = root / "src" / "Mod" / "Test"
    skip = {"__init__", "Init", "InitGui", "unittestgui", "Metadata"}
    return sorted(
        p.stem for p in test_dir.glob("*.py")
        if p.stem not in skip and not p.name.startswith("_")
    )


def render(outcomes: list[dict]) -> str:
    lines: list[str] = []
    lines.append("FreeCAD conformance report (M3d)")
    lines.append("=" * 60)

    grand = {"pass": 0, "fail": 0, "error": 0, "skip": 0, "load_error": 0}
    for o in outcomes:
        if not o["loaded"]:
            grand["load_error"] += 1
            lines.append(f"\n{o['file']}: LOAD ERROR — {o['reason']}")
            continue
        t = o["totals"]
        for k in grand:
            if k in t:
                grand[k] += t[k]
        lines.append(
            f"\n{o['file']}: {sum(t.values())} tests "
            f"(pass {t['pass']}, fail {t['fail']}, error {t['error']}, skip {t['skip']})"
        )
        for test_id, status, msg in o["tests"]:
            if status == "pass":
                continue
            lines.append(f"  [{status.upper()}] {test_id}")
            if msg:
                lines.append(f"      {msg}")

    lines.append("\n" + "=" * 60)
    lines.append(
        "summary: "
        f"{grand['pass']} passed, {grand['fail']} failed, "
        f"{grand['error']} errored, {grand['skip']} skipped, "
        f"{grand['load_error']} files failed to load"
    )
    return "\n".join(lines)


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Run upstream tests against our FreeCAD.")
    parser.add_argument("--root", required=True, help="path to the freecad-upstream checkout")
    parser.add_argument("--files", nargs="*", default=DEFAULT_FILES, help="test file stems to run")
    parser.add_argument("--list", action="store_true", help="list candidate test files and exit")
    args = parser.parse_args(argv)

    root = Path(args.root).resolve()
    test_dir = root / "src" / "Mod" / "Test"

    if args.list:
        print("\n".join(list_candidates(root)))
        return 0

    if str(PYTHON_DIR) not in sys.path:
        sys.path.insert(0, str(PYTHON_DIR))
    if str(test_dir) not in sys.path:
        sys.path.insert(0, str(test_dir))

    outcomes = []
    for stem in args.files:
        path = test_dir / f"{stem}.py"
        if not path.is_file():
            print(f"warning: not found: {path}", file=sys.stderr)
            continue
        outcomes.append(run_file(path))

    print(render(outcomes))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
