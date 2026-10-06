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
    python3 tools/conformance.py --root ../freecad-upstream --part      # Part workbench tests
    python3 tools/conformance.py --root ../freecad-upstream --list      # list candidates

The `--part` group needs an image that links the `Part` module (it imports `Part`
and `FreeCAD` and they must share one core), which the standalone `ferrocad.abi3.so`
does not. Run it through
`cargo test -p ferrocad_part_py --test part_conformance -- --nocapture` (set
`FERROCAD_UPSTREAM`), which supplies the one image and calls this harness.
"""

from __future__ import annotations

import argparse
import importlib.util
import re
import sys
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]   # ferrocad project root
PYTHON_DIR = REPO / "python"

DEFAULT_FILES = [
    "Document",
    "StringHasher",
    "UnitTests",
    "UnicodeTests",
    "TestApp",
    "BaseTests",
    "TestIntPairList",
    "FreeCADInitTests",
]

# The Part workbench's App-level tests. They live beside the module rather than in
# `src/Mod/Test`, and they import `Part` as well as `FreeCAD`, so they only run in
# an image that links both (the `Part` module must share one core with the
# bindings; see `crates/ferrocad_part_py/tests/part_conformance.rs`).
PART_DIRS = ["src/Mod/Part/parttests", "src/Mod/Part"]
PART_FILES = [
    "BRep_tests",
    "Geom2d_tests",
    "regression_tests",
    "TopoShapeTest",
    "TopoShapeListTest",
    "TestPartMirror",
    "TestFaceMakerUnifiedPlanar",
    "TestFaceMakerUnifiedNonPlanar",
    "ColorPerFaceTest",
    "ColorTransparencyTest",
    "TestPartApp",
]


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
    """Import a test file as a module; return (module, None) or (None, error).

    The module is registered in ``sys.modules`` before its body runs, exactly as
    a normal import would. FreeCAD persists Python proxy objects by referencing
    their defining module by name, so an unregistered module makes proxy
    save/restore tests fail spuriously.
    """
    spec = importlib.util.spec_from_file_location(path.stem, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    try:
        spec.loader.exec_module(module)
    except Exception as exc:  # noqa: BLE001 - we want any import failure
        sys.modules.pop(spec.name, None)
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
    parser.add_argument("--files", nargs="*", default=None, help="test file stems to run")
    parser.add_argument("--list", action="store_true", help="list candidate test files and exit")
    parser.add_argument(
        "--part",
        action="store_true",
        help="run the Part workbench tests (needs an image that links `Part`)",
    )
    args = parser.parse_args(argv)

    root = Path(args.root).resolve()
    test_dir = root / "src" / "Mod" / "Test"

    if args.list:
        print("\n".join(list_candidates(root)))
        return 0

    if args.part:
        dirs = [root / d for d in PART_DIRS]
        stems = args.files if args.files else PART_FILES
    else:
        dirs = [test_dir]
        stems = args.files if args.files else DEFAULT_FILES

    # `src/Mod/Part` must be importable as a package root: the Part tests do
    # `from parttests.X import Y`.
    for d in dirs:
        if str(d) not in sys.path:
            sys.path.insert(0, str(d))
    if str(PYTHON_DIR) not in sys.path:
        sys.path.insert(0, str(PYTHON_DIR))

    outcomes = []
    for stem in stems:
        path = next((d / f"{stem}.py" for d in dirs if (d / f"{stem}.py").is_file()), None)
        if path is None:
            print(f"warning: not found: {stem}", file=sys.stderr)
            continue
        outcomes.append(run_file(path))

    print(render(outcomes))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
