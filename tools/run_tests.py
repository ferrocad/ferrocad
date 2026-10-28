#!/usr/bin/env python3
"""Run the Python suites against an **explicitly named** extension set.

``unittest discover`` runs against whatever ``.so`` files happen to be in
``python/``, so a test that needs ``Part`` passes on a developer checkout that
built ``ferrocad_combined.abi3.so`` and fails in CI, which builds only the
primary extension. This runner removes the inference:

* each run names the workbench modules it is **provided** (``--extension``),
  and ``extensions.install`` hides every workbench module it was not given;
* ``extensions.install(modules)`` runs before discovery in a **fresh
  subprocess**, so imports (and any module-level caching) cannot leak between
  extension sets;
* ``contracts/extension-sets.json`` records which suite a module belongs to, so
  adding a workbench dependency to a base module changes a reviewed diff.

Usage::

    python3 tools/run_tests.py --check            # contract check, no imports
    python3 tools/run_tests.py --extension base   # the base suites
    python3 tools/run_tests.py --extension workbenches   # every suite
    python3 tools/run_tests.py --write-contract   # re-derive the contract

An *error* is a failure; a *skip* is not. In the ``base`` set a module may pass
or skip, never error. ``--write-contract`` therefore classifies a module as
``workbenches`` when it passes with the workbench modules provided and passes
nothing in ``base``. A module that passes nothing in *either* set is ``base``:
it skips for an optional artifact or checkout (``fc_gen``, the SimpleCAD tree),
which is a different axis. So is a module with no ``unittest`` tests at all,
such as ``tests/test_fc_core.py``, which CI runs as a script.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

import extensions

REPO = Path(__file__).resolve().parents[1]
TESTS = REPO / "tests"
TOOLS = REPO / "tools"
PYTHON_DIR = REPO / "python"
CONTRACT = REPO / "contracts" / "extension-sets.json"

SCHEMA = 2
SUITES = tuple(extensions.PRESETS)
_COMBINED_GLOB = "ferrocad_combined.abi3.*"

_BUILD_HINT = (
    "./build.sh (or: cargo build --offline -p ferrocad_combined_py "
    "--features ferrocad_combined_py/extension-module && "
    "cp target/debug/libferrocad_combined_py.so python/ferrocad_combined.abi3.so)"
)


# ---------------------------------------------------------------------------
# Discovery and contract validation (no project imports, no build)
# ---------------------------------------------------------------------------
def discovered_modules() -> list[str]:
    """Every ``tests/test_*.py`` module, by importable name."""

    return sorted(path.stem for path in TESTS.glob("test_*.py") if path.is_file())


def _module_list(value, where: str, problems: list[str]) -> list[str]:
    if not isinstance(value, list) or not all(isinstance(item, str) for item in value):
        problems.append(f"{where} must be a list of module names")
        return []
    return value


def validate_contract(contract, modules: list[str]) -> list[str]:
    """Return the problems with ``contract``; an empty list means it is valid."""

    if not isinstance(contract, dict):
        return [f"{CONTRACT.name} must be a JSON object"]

    problems: list[str] = []

    presets = contract.get("presets")
    if not isinstance(presets, dict):
        problems.append("`presets` must be an object mapping preset to modules")
    else:
        known = set(extensions.PRESETS)
        unknown = sorted(set(presets) - known)
        if unknown:
            problems.append(f"unknown preset(s) in `presets`: {', '.join(unknown)}")
        missing = sorted(known - set(presets))
        if missing:
            problems.append(f"missing preset(s) in `presets`: {', '.join(missing)}")
        for name, expected in extensions.PRESETS.items():
            if name not in presets:
                continue
            listed = _module_list(presets[name], f"`presets.{name}`", problems)
            if isinstance(presets[name], list) and listed != sorted(expected):
                problems.append(
                    f"`presets.{name}` must list exactly "
                    f"{sorted(expected)}, not {listed}"
                )

    suites = contract.get("suites")
    if not isinstance(suites, dict):
        problems.append("`suites` must be an object mapping suite to modules")
        return problems

    unknown_suites = sorted(set(suites) - set(SUITES))
    if unknown_suites:
        problems.append(
            "entries for unknown suite(s): " + ", ".join(unknown_suites)
        )
    for suite in SUITES:
        if suite not in suites:
            problems.append(f"missing suite `{suite}`")

    seen: dict[str, str] = {}
    known_modules = set(modules)
    for suite in SUITES:
        if suite not in suites:
            continue
        listed = _module_list(suites[suite], f"`suites.{suite}`", problems)
        for name in listed:
            if name in seen:
                problems.append(
                    f"`{name}` is listed twice (`{seen[name]}` and `{suite}`)"
                )
            else:
                seen[name] = suite
            if name not in known_modules:
                problems.append(f"`{name}` is listed in `{suite}` but does not exist")

    for name in modules:
        if name not in seen:
            problems.append(f"`{name}` is not listed in the contract")

    return problems


def load_contract() -> dict:
    return json.loads(CONTRACT.read_text())


def check() -> int:
    modules = discovered_modules()
    if not CONTRACT.is_file():
        print(
            f"error: {CONTRACT.relative_to(REPO)} is missing; "
            "run `python3 tools/run_tests.py --write-contract`",
            file=sys.stderr,
        )
        return 2
    try:
        contract = load_contract()
    except json.JSONDecodeError as error:
        print(f"error: {CONTRACT.relative_to(REPO)}: {error}", file=sys.stderr)
        return 2

    problems = validate_contract(contract, modules)
    if problems:
        for problem in problems:
            print(f"error: {problem}", file=sys.stderr)
        print(
            "error: the extension-set contract is out of date; run "
            "`python3 tools/run_tests.py --write-contract`",
            file=sys.stderr,
        )
        return 1

    suites = contract["suites"]
    print(
        "extension-set contract is current "
        f"({len(modules)} modules: {len(suites['base'])} base, "
        f"{len(suites['workbenches'])} workbenches)"
    )
    return 0


# ---------------------------------------------------------------------------
# Running one extension set
# ---------------------------------------------------------------------------
def _combined_artifact() -> Path | None:
    matches = sorted(PYTHON_DIR.glob(_COMBINED_GLOB))
    return matches[0] if matches else None


def _child_env() -> dict[str, str]:
    """The environment for a run: ``python/`` + ``tools/`` importable, OCCT found."""

    env = dict(os.environ)
    paths = [str(PYTHON_DIR), str(TOOLS)]
    existing = env.get("PYTHONPATH")
    if existing:
        paths.append(existing)
    env["PYTHONPATH"] = os.pathsep.join(paths)

    # The combined extension links OCCT, which ships no RUNPATH. `build.sh`
    # exports this; a bare `run_tests.py` invocation should work too.
    occt_lib = REPO / ".occt" / "prefix" / "lib"
    if occt_lib.is_dir():
        variable = (
            "DYLD_LIBRARY_PATH" if sys.platform == "darwin" else "LD_LIBRARY_PATH"
        )
        previous = env.get(variable)
        env[variable] = str(occt_lib) + (os.pathsep + previous if previous else "")
    return env


def _module_record(name: str) -> dict:
    return {
        "module": name,
        "tests": 0,
        "skipped": 0,
        "failures": 0,
        "errors": 0,
        "load_error": None,
    }


def _child(request_path: Path) -> int:
    """Run the request's modules against its extension set. Subprocess entry point only."""

    import unittest

    request = json.loads(request_path.read_text())
    provided = frozenset(request["extensions"])
    verbosity = request["verbosity"]
    modules = request["modules"]

    # Install the extension set *before* discovery/importing, so a module that
    # needs a hidden workbench module cannot be satisfied by a stale
    # ``sys.modules`` entry.
    extensions.install(provided)

    if str(TESTS) not in sys.path:
        sys.path.insert(0, str(TESTS))

    loader = unittest.TestLoader()
    runner = unittest.TextTestRunner(
        stream=sys.stderr, verbosity=verbosity, descriptions=False
    )

    # Import every module before running any test, which is what
    # ``unittest discover`` does. Running one module at a time would let a test
    # that mutates ``sys.modules`` change a *later* module's import-time
    # decisions, so the extension set would not be the only variable.
    records = [_module_record(name) for name in modules]
    suites = {}
    for record in records:
        name = record["module"]
        try:
            suites[name] = loader.loadTestsFromName(name)
        except Exception as error:  # noqa: BLE001 - import-time failure
            record["load_error"] = f"{type(error).__name__}: {error}"
            record["errors"] = 1
            print(
                f"error: could not import {name}: {record['load_error']}",
                file=sys.stderr,
            )

    for record in records:
        suite = suites.get(record["module"])
        if suite is None:
            continue
        print(f"== {record['module']} ==", file=sys.stderr)
        result = runner.run(suite)
        record["tests"] = result.testsRun
        record["skipped"] = len(result.skipped)
        record["failures"] = len(result.failures)
        record["errors"] = len(result.errors)

    Path(request["result"]).write_text(
        json.dumps({"extensions": sorted(provided), "modules": records})
    )
    failed = any(
        record["errors"] or record["failures"] or record["load_error"]
        for record in records
    )
    return 1 if failed else 0


def _run(
    provided: frozenset[str], modules: list[str], verbosity: int
) -> tuple[list[dict] | None, int]:
    """Run ``modules`` in one fresh subprocess and return its records."""

    with tempfile.TemporaryDirectory(prefix="ferrocad-tests-") as directory:
        directory_path = Path(directory)
        result_path = directory_path / "result.json"
        request_path = directory_path / "request.json"
        request_path.write_text(
            json.dumps(
                {
                    "extensions": sorted(provided),
                    "verbosity": verbosity,
                    "modules": modules,
                    "result": str(result_path),
                }
            )
        )
        argv = [
            sys.executable,
            str(Path(__file__).resolve()),
            "--_child",
            str(request_path),
        ]
        process = subprocess.run(argv, cwd=REPO, env=_child_env())
        if not result_path.is_file():
            label = ", ".join(sorted(provided)) or "base"
            print(
                f"error: the {label!r} extension-set process exited with "
                f"{process.returncode} before reporting results",
                file=sys.stderr,
            )
            return None, process.returncode or 1
        return json.loads(result_path.read_text())["modules"], process.returncode


def _status(record: dict) -> str:
    if record["load_error"]:
        return "load-error"
    if record["errors"]:
        return "error"
    if record["failures"]:
        return "fail"
    if record["tests"] == 0:
        return "empty"
    if record["skipped"] == record["tests"]:
        return "skipped"
    return "ok"


def _summarise(label: str, records: list[dict]) -> None:
    print(f"\nextension set: {label}")
    print(f"{'module':<34} {'tests':>6} {'skip':>6} {'fail':>6} {'err':>6}  status")
    for record in records:
        print(
            f"{record['module']:<34} {record['tests']:>6} {record['skipped']:>6} "
            f"{record['failures']:>6} {record['errors']:>6}  {_status(record)}"
        )
    total_tests = sum(record["tests"] for record in records)
    total_skipped = sum(record["skipped"] for record in records)
    total_failures = sum(record["failures"] for record in records)
    total_errors = sum(record["errors"] for record in records)
    print(
        f"{'total':<34} {total_tests:>6} {total_skipped:>6} "
        f"{total_failures:>6} {total_errors:>6}"
    )


def run_set(modules: frozenset[str], verbosity: int) -> int:
    if modules and _combined_artifact() is None:
        artifact = _COMBINED_GLOB.replace(".*", ".so")
        provided = ", ".join(sorted(modules))
        print(
            f"error: the extension set {provided!r} needs python/{artifact}, "
            "which is not built.\n"
            f"       Run {_BUILD_HINT}.",
            file=sys.stderr,
        )
        return 2

    if not CONTRACT.is_file():
        print(
            f"error: {CONTRACT.relative_to(REPO)} is missing; "
            "run `python3 tools/run_tests.py --write-contract`",
            file=sys.stderr,
        )
        return 2
    try:
        contract = load_contract()
    except json.JSONDecodeError as error:
        print(f"error: {CONTRACT.relative_to(REPO)}: {error}", file=sys.stderr)
        return 2
    problems = validate_contract(contract, discovered_modules())
    if problems:
        for problem in problems:
            print(f"error: {problem}", file=sys.stderr)
        print(
            "error: the extension-set contract is out of date; run "
            "`python3 tools/run_tests.py --write-contract`",
            file=sys.stderr,
        )
        return 2

    suites = contract["suites"]
    selected = list(suites["base"])
    if modules:
        selected += list(suites["workbenches"])
    else:
        not_run = len(suites["workbenches"])
        if not_run:
            print(
                f"note: {not_run} workbench module(s) are not run without an "
                "extension; provide them with `--extension Part --extension Mesh "
                "--extension MeshPart` (or `--extension workbenches`)",
                file=sys.stderr,
            )

    label = ", ".join(sorted(modules)) or "nothing (base)"
    records, code = _run(modules, selected, verbosity)
    if records is None:
        return code
    _summarise(label, records)

    failed = [
        record
        for record in records
        if record["errors"] or record["failures"] or record["load_error"]
    ]
    if failed:
        print(
            f"\nerror: {len(failed)} module(s) errored or failed in the "
            f"extension set {label!r}",
            file=sys.stderr,
        )
        return 1
    return 0


# ---------------------------------------------------------------------------
# Deriving the contract
# ---------------------------------------------------------------------------
def _passes_any(record: dict) -> bool:
    return (
        record["load_error"] is None
        and not record["errors"]
        and not record["failures"]
        and record["tests"] > record["skipped"]
    )


def write_contract(verbosity: int) -> int:
    if _combined_artifact() is None:
        artifact = _COMBINED_GLOB.replace(".*", ".so")
        print(
            "error: `--write-contract` runs every module with the workbenches "
            f"extension set, so python/{artifact} must exist.\n"
            f"       Run {_BUILD_HINT}.",
            file=sys.stderr,
        )
        return 2

    modules = discovered_modules()
    print(f"deriving the contract from {len(modules)} modules\n", file=sys.stderr)

    workbenches, code = _run(extensions.preset("workbenches"), modules, verbosity)
    if workbenches is None:
        return code
    broken = [
        record
        for record in workbenches
        if record["errors"] or record["failures"] or record["load_error"]
    ]
    if broken:
        _summarise("workbenches", workbenches)
        print(
            "\nerror: the workbenches extension set is not green, so the contract "
            "cannot be derived; fix these modules first",
            file=sys.stderr,
        )
        return 1

    base, code = _run(extensions.preset("base"), modules, verbosity)
    if base is None:
        return code

    workbenches_by_module = {record["module"]: record for record in workbenches}
    suites: dict[str, list[str]] = {suite: [] for suite in SUITES}
    for record in base:
        # A module that passes any test in the base set is a base module. A
        # module that passes nothing in *either* set is not workbench-dependent:
        # it selects for an optional artifact or checkout (fc_gen, the SimpleCAD
        # tree). Workbenches means it works with the workbench modules and
        # cannot in `base`.
        if _passes_any(record) or not _passes_any(
            workbenches_by_module[record["module"]]
        ):
            suites["base"].append(record["module"])
        else:
            suites["workbenches"].append(record["module"])

    for suite in suites:
        suites[suite].sort()

    contract = {
        "schema": SCHEMA,
        "presets": {
            name: sorted(provided) for name, provided in extensions.PRESETS.items()
        },
        "suites": suites,
    }
    text = json.dumps(contract, indent=2) + "\n"
    old = CONTRACT.read_text() if CONTRACT.is_file() else None
    if old == text:
        print(
            f"\n{CONTRACT.relative_to(REPO)} is unchanged "
            f"({len(suites['base'])} base, {len(suites['workbenches'])} workbenches)"
        )
        return 0

    CONTRACT.parent.mkdir(parents=True, exist_ok=True)
    CONTRACT.write_text(text)
    print(
        f"\nwrote {CONTRACT.relative_to(REPO)} "
        f"({len(suites['base'])} base, {len(suites['workbenches'])} workbenches)"
    )
    return 0


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------
def _resolve_extension_set(parser, values: list[str]) -> frozenset[str]:
    """The workbench modules the run is provided, from the ``--extension`` flags."""

    if not values:
        return extensions.preset("base")

    presets = [value for value in values if value in extensions.PRESETS]
    if presets:
        if len(values) > 1:
            parser.error(
                f"--extension {presets[0]} is a preset and cannot be combined "
                "with other --extension values; name the modules it provides "
                "instead"
            )
        return extensions.preset(presets[0])

    try:
        return extensions.provided(frozenset(values))
    except ValueError as error:
        parser.error(str(error))


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__.splitlines()[0],
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    group = parser.add_mutually_exclusive_group()
    group.add_argument(
        "--check",
        action="store_true",
        help="validate contracts/extension-sets.json against tests/test_*.py "
        "(no imports)",
    )
    group.add_argument(
        "--extension",
        action="append",
        metavar="NAME",
        help="a workbench module this run is provided (repeatable), or a preset "
        f"({', '.join(sorted(extensions.PRESETS))})",
    )
    group.add_argument(
        "--write-contract",
        action="store_true",
        help="run each module in both extension sets and re-derive the contract",
    )
    parser.add_argument(
        "-v", "--verbose", action="store_true", help="verbose unittest output"
    )
    parser.add_argument(
        "--_child",
        metavar="REQUEST",
        help="internal: run one extension set in a subprocess",
    )
    args = parser.parse_args(argv)

    if args.check:
        return check()
    if args.write_contract:
        return write_contract(2 if args.verbose else 1)
    if args._child:
        return _child(Path(args._child))

    if not args.extension:
        parser.error("one of --check, --extension or --write-contract is required")
    verbosity = 2 if args.verbose else 1
    return run_set(_resolve_extension_set(parser, args.extension), verbosity)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
