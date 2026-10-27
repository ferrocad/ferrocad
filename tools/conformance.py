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
    python3 tools/conformance.py --root ../freecad-upstream --draft     # headless Draft tests
    python3 tools/conformance.py --root ../freecad-upstream --list      # list candidates
    python3 tools/conformance.py --simplecad --list                     # SimpleCAD unit stems
    python3 tools/conformance.py --simplecad                            # run SimpleCAD unit tests

The one-time setup (the sparse `freecad-upstream` checkout, the extension build) and
the current baseline are in ``docs/workbenches/conformance.md``.

The `--part` group needs an image that links the `Part` module (it imports `Part`
and `FreeCAD` and they must share one core), which the standalone `ferrocad.abi3.so`
does not. Run it through
`cargo test -p ferrocad_part_py --test part_conformance -- --nocapture` (set
`FERROCAD_UPSTREAM`), which supplies the one image and calls this harness.

The `--simplecad` group has its own source root, a sibling checkout outside version
control (default ``../simplecad``, or ``SIMPLECAD_DIR`` / ``--simplecad-root``). It
runs ``tests/unit/test_*.py`` unchanged and self-skips when the checkout is absent,
so normal CI stays hermetic. S0 only hooks it up; S1 records the baseline.
"""

from __future__ import annotations

import argparse
import importlib.util
import os
import re
import subprocess
import sys
import unittest
from collections.abc import Callable
from dataclasses import dataclass
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
    "TestLinkArrayPoint",
    "TestLinkArrayLinear",
    "TestLinkArrayPath",
    "TestLinkArrayCircular",
]

# The headless half of the Draft workbench. `src/Mod/Draft` is the package root
# (`drafttests`, `draftutils`, …); `src/Ext` provides `freecad.deprecation`. The
# GUI-adjacent files (`test_*_gui`, `test_import_tools`) are deliberately absent:
# they import `draftguitools`/`FreeCADGui`.
DRAFT_DIRS = [
    "src/Mod/Draft/drafttests",
    "src/Mod/Draft",
    "src/Ext",
]
DRAFT_FILES = [
    "test_base",
    "test_grid_settings",
    "test_pivy",
    "test_airfoildat",
    "test_import",
    # M2/M3 modules that now run green in this group (they need the combined
    # App+Part image, as `test_import` already did). `test_modification` runs
    # 31/0/2 but stays out: its two remaining errors need `Sketcher` and
    # `TechDraw::DrawPage`, which this image does not ship. The import modules
    # are green where they can be: see docs/workbenches/draft-tools.md.
    "test_creation",
    "test_array",
    "test_dwg",
    "test_oca",
    # SVG export runs green once `Part.OCC_VERSION` and the conic parameter
    # ranges exist; its one skip is honest (the BIM module is not shipped).
    "test_svg",
]

# `src/Mod/Test` also holds package plumbing; the App group's `--list` has always
# excluded these stems.
APP_SKIP = {"__init__", "Init", "InitGui", "unittestgui", "Metadata"}


@dataclass(frozen=True)
class Group:
    """A named set of test files and the source root(s) they come from.

    ``roots`` are both the directories searched for ``<stem>.py`` and the
    directories inserted on ``sys.path``. ``import_roots`` are extra directories
    that only need to be importable (the SimpleCAD checkout root, so
    ``from simplecad.core import ...`` resolves). ``stems`` is a fixed list; an
    empty tuple means the group discovers its files with ``discover``.

    Keeping the roots in the descriptor is what lets the harness stop assuming a
    single ``--root``: the App/Part/Draft groups point at one FreeCAD checkout,
    while SimpleCAD points at a sibling checkout.
    """

    name: str
    roots: tuple[Path, ...]
    stems: tuple[str, ...]
    discover: Callable[[], list[str]]
    import_roots: tuple[Path, ...] = ()


def simplecad_path(explicit: str | None = None) -> Path:
    """Resolve the SimpleCAD checkout: CLI, then environment, then ``../simplecad``."""
    if explicit:
        return Path(explicit).expanduser().resolve()
    env = os.environ.get("SIMPLECAD_DIR") or os.environ.get("FERROCAD_SIMPLECAD")
    if env:
        return Path(env).expanduser().resolve()
    return (REPO.parent / "simplecad").resolve()


def app_group(root: Path) -> Group:
    return Group(
        "app",
        (root / "src" / "Mod" / "Test",),
        tuple(DEFAULT_FILES),
        lambda: list_candidates(root),
    )


def part_group(root: Path) -> Group:
    return Group(
        "part",
        tuple(root / d for d in PART_DIRS),
        tuple(PART_FILES),
        lambda: list_candidates(root),
    )


def draft_group(root: Path) -> Group:
    return Group(
        "draft",
        tuple(root / d for d in DRAFT_DIRS),
        tuple(DRAFT_FILES),
        lambda: list_candidates(root),
    )


def simplecad_group(root: Path) -> Group:
    unit = root / "tests" / "unit"
    return Group(
        "simplecad",
        (unit,),
        (),
        lambda: sorted(p.stem for p in unit.glob("test_*.py")),
        import_roots=(root,),
    )


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

    def addSubTest(self, test, subtest, err):
        """Record a failing subtest.

        ``unittest`` calls this for every ``with self.subTest(...)`` block. The base
        implementation only appends to ``failures``/``errors``, which this collector
        does not report, so without this override a subtest failure disappears and
        the parent test is never recorded as a success either. Passing subtests are
        not recorded: when all of them pass the parent is reported as a success.
        """
        if err is None:
            return
        kind = "fail" if issubclass(err[0], test.failureException) else "error"
        self.records.append((subtest.id(), kind, _short(err)))


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


def upstream_revision(root: Path) -> str:
    """The checkout's ``HEAD``, so every report is tied to a revision."""
    try:
        out = subprocess.run(
            ["git", "--no-pager", "-C", str(root), "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError):
        return "unknown"
    return out.stdout.strip() or "unknown"


def list_candidates(root: Path) -> list[str]:
    test_dir = root / "src" / "Mod" / "Test"
    return sorted(
        p.stem for p in test_dir.glob("*.py")
        if p.stem not in APP_SKIP and not p.name.startswith("_")
    )


def package_version(root: Path) -> str | None:
    """The checkout's ``package.xml`` version, when it has one."""
    try:
        manifest = (root / "package.xml").read_text(encoding="utf-8")
    except OSError:
        return None
    match = re.search(r"<version>\s*([^<]+?)\s*</version>", manifest)
    return match.group(1) if match else None


def render(
    outcomes: list[dict],
    root: Path | None = None,
    *,
    source_label: str = "upstream",
    source_version: str | None = None,
) -> str:
    lines: list[str] = []
    lines.append("FreeCAD conformance report (M3d)")
    if root is not None:
        version = f", version {source_version}" if source_version else ""
        lines.append(
            f"{source_label}: {root} @ {upstream_revision(root)}{version}"
        )
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
    parser.add_argument(
        "--root",
        default=None,
        help="path to the freecad-upstream checkout (required unless --simplecad)",
    )
    parser.add_argument("--files", nargs="*", default=None, help="test file stems to run")
    parser.add_argument("--list", action="store_true", help="list candidate test files and exit")
    parser.add_argument(
        "--part",
        action="store_true",
        help="run the Part workbench tests (needs an image that links `Part`)",
    )
    parser.add_argument(
        "--draft",
        action="store_true",
        help="run the headless Draft workbench tests",
    )
    parser.add_argument(
        "--simplecad",
        action="store_true",
        help="run the SimpleCAD tests/unit/test_*.py modules",
    )
    parser.add_argument(
        "--simplecad-root",
        metavar="PATH",
        default=None,
        help=(
            "SimpleCAD checkout (default: $SIMPLECAD_DIR, $FERROCAD_SIMPLECAD "
            "or ../simplecad)"
        ),
    )
    args = parser.parse_args(argv)

    # `--list` has always ignored `--part`/`--draft`; keep that contract and give
    # `--simplecad` its own listing, which is also the only listing that can work
    # without `--root`.
    if args.list:
        if args.simplecad:
            simple_root = simplecad_path(args.simplecad_root)
            if not simple_root.is_dir():
                print(
                    f"skipped: SimpleCAD checkout not found at {simple_root} "
                    "(set SIMPLECAD_DIR, FERROCAD_SIMPLECAD or --simplecad-root)"
                )
                return 0
            print("\n".join(simplecad_group(simple_root).discover()))
            return 0
        if args.root is None:
            parser.error("--root is required unless --simplecad is given")
        print("\n".join(list_candidates(Path(args.root).resolve())))
        return 0

    if args.part and args.draft:
        parser.error("--part and --draft are separate groups; run one at a time")
    if args.simplecad and (args.part or args.draft):
        parser.error("--simplecad is a separate group; run it alone")

    if args.simplecad:
        root = simplecad_path(args.simplecad_root)
        if not root.is_dir():
            print(
                f"skipped: SimpleCAD checkout not found at {root} "
                "(set SIMPLECAD_DIR, FERROCAD_SIMPLECAD or --simplecad-root)"
            )
            return 0
        group = simplecad_group(root)
    else:
        if args.root is None:
            parser.error("--root is required unless --simplecad is given")
        root = Path(args.root).resolve()
        if args.part:
            group = part_group(root)
        elif args.draft:
            group = draft_group(root)
        else:
            group = app_group(root)

    stems = list(args.files) if args.files else list(group.stems or group.discover())

    # The workbench package roots must be importable: the Part tests do
    # `from parttests.X import Y`, and Draft does `from drafttests import
    # auxiliary` and `from freecad.deprecation import deprecated`. SimpleCAD
    # needs its checkout root (`from simplecad.core import ...`) and `tests/unit`
    # (`from test_dense_deletion import dense_box`).
    #
    # `python/` goes on the path *first*, then the group roots are inserted ahead
    # of it. The harness measures upstream code, so an upstream package must win
    # over a FerroCAD shim of the same name: `Draft` is the live case (the Part
    # group needs `python/Draft` to load `TestPartMirror`, while the Draft group
    # must run `src/Mod/Draft/Draft.py`). Our own modules (`FreeCAD`, `Part`, …)
    # are built-ins, so path order does not affect them.
    if str(PYTHON_DIR) not in sys.path:
        sys.path.insert(0, str(PYTHON_DIR))
    for d in (*group.roots, *group.import_roots):
        if str(d) not in sys.path:
            sys.path.insert(0, str(d))

    outcomes = []
    for stem in stems:
        path = next(
            (d / f"{stem}.py" for d in group.roots if (d / f"{stem}.py").is_file()),
            None,
        )
        if path is None:
            print(f"warning: not found: {stem}", file=sys.stderr)
            continue
        outcomes.append(run_file(path))

    print(
        render(
            outcomes,
            root,
            source_label="simplecad" if args.simplecad else "upstream",
            source_version=package_version(root) if args.simplecad else None,
        )
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
