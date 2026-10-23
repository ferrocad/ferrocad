"""D1-D3: the headless imports must survive with every real Qt distribution blocked.

Each probe runs in a **fresh subprocess** whose first action is to install a
``sys.meta_path`` finder that raises for ``PySide``, ``PySide2``, ``PySide6``,
``shiboken``, ``shiboken2`` and ``shiboken6``. The order matters: the blocker is
in place before anything under test is imported, so an accidental real-Qt import
fails at the import site instead of being masked by an earlier load.

The repository's own ``python/PySide`` **is** the shim under test. The guard
blocks all six names including ``PySide``, but in the Draft probe it whitelists
the *one* package whose ``__init__.py`` lives under ``python/PySide`` and then
asserts that the loaded module is the pure-Python stub (``PySide.USING_STUB``).
An installed Qt5 ``PySide`` elsewhere on ``sys.path`` is still refused:

* the **facade** probe (D1) blocks all six names outright — ``FreeCAD``,
  ``FreeCADGui`` and ``ferrocad_shell`` must not need any PySide, including the
  shim — and proves the blocker works by asserting ``import PySide`` fails;
* the **Draft** probe (D3) blocks all six but admits the repository's shim, then
  asserts that the loaded module is the stub and that nothing else under a
  blocked name appeared;
* the **shim** probe runs with no blocker at all and asserts that importing
  ``PySide`` does not drag in the installed ``PySide6``/``shiboken6``.

D3 additionally pins ``params.get_param("textheight") == 3.5`` and 160
``Mod/Draft`` entries, so the resource reader is exercised, not just imported.

The contract group at the bottom runs ``tools/draft_contract.py`` under the same
blocker: Q0d must be derivable from XML and text alone.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
PYTHON_DIR = REPO / "python"
TOOL = REPO / "tools" / "draft_contract.py"
CONTRACT = REPO / "contracts" / "draft-forms.json"
UPSTREAM = Path(os.environ.get("FERROCAD_UPSTREAM", REPO.parent / "freecad-upstream"))
DRAFT = UPSTREAM / "src" / "Mod" / "Draft"
UPSTREAM_EXT = UPSTREAM / "src" / "Ext"

#: Top-level module names the guard refuses. The Draft probe carves out the
#: repository's own ``PySide`` package (see the module docstring).
BLOCKED = ("PySide", "PySide2", "PySide6", "shiboken", "shiboken2", "shiboken6")

_RESULT_PREFIX = "PROBE_RESULT "

_PROBE = r'''
import importlib.abc
import importlib.machinery
import json
import os
import sys

BLOCKED = {"PySide", "PySide2", "PySide6", "shiboken", "shiboken2", "shiboken6"}
SHIM_ROOT = os.path.abspath(os.path.join(os.environ["FERROCAD_ISOLATION_PYTHON_DIR"], "PySide"))
mode = sys.argv[1]


class BlockQt(importlib.abc.MetaPathFinder):
    def find_spec(self, name, path=None, target=None):
        top = name.split(".")[0]
        if top not in BLOCKED:
            return None
        if mode == "draft" and top == "PySide":
            if name == "PySide":
                # Admit only the repository's own pure-Python package.
                spec = importlib.machinery.PathFinder.find_spec(name, None)
                origin = getattr(spec, "origin", "") or ""
                if spec is not None and origin.startswith(SHIM_ROOT):
                    return spec
            elif any(
                os.path.abspath(str(entry)).startswith(SHIM_ROOT)
                for entry in (path or ())
            ):
                # A real file inside the admitted package (``_qt_stub``); the
                # normal PathFinder must handle it.
                return None
        raise ModuleNotFoundError(
            f"{name} is blocked by the FerroCAD Qt isolation guard", name=name
        )


if mode != "shim-unblocked":
    sys.meta_path.insert(0, BlockQt())

# Prove the blocker is installed before trusting the imports below.
if mode == "facade":
    for blocked_name in ("PySide", "PySide6", "shiboken6"):
        try:
            __import__(blocked_name)
        except ModuleNotFoundError:
            pass
        else:
            raise AssertionError(f"the guard failed to block {blocked_name}")

import FreeCAD
import FreeCADGui
import ferrocad_shell

payload = {"gui_up": getattr(FreeCAD, "GuiUp", None)}

if mode == "draft":
    from draftutils import params
    import PySide

    payload["draft"] = {
        "entries": len(params.PARAM_DICT["Mod/Draft"]),
        "textheight": params.get_param("textheight"),
        "shim": bool(getattr(PySide, "USING_STUB", False)),
        "origin": PySide.__file__,
    }
    for blocked_name in ("PySide2", "PySide6", "shiboken", "shiboken6"):
        try:
            __import__(blocked_name)
        except ModuleNotFoundError:
            pass
        else:
            raise AssertionError(f"the guard failed to block {blocked_name}")
elif mode == "shim-unblocked":
    import PySide

    payload["shim"] = bool(getattr(PySide, "USING_STUB", False))

payload["modules"] = sorted(sys.modules)
print("PROBE_RESULT " + json.dumps(payload))
'''

_TOOL_WRAPPER = r'''
import importlib.abc
import runpy
import sys

BLOCKED = {"PySide", "PySide2", "PySide6", "shiboken", "shiboken2", "shiboken6"}


class BlockQt(importlib.abc.MetaPathFinder):
    def find_spec(self, name, path=None, target=None):
        top = name.split(".")[0]
        if top in BLOCKED:
            raise ModuleNotFoundError(
                f"{name} is blocked by the FerroCAD Qt isolation guard", name=name
            )
        return None


sys.meta_path.insert(0, BlockQt())
tool = sys.argv[1]
sys.argv[1:] = sys.argv[2:]
runpy.run_path(tool, run_name="__main__")
'''


def _probe_env() -> dict:
    env = dict(os.environ)
    env["PYTHONPATH"] = os.pathsep.join(
        [str(PYTHON_DIR), str(DRAFT), str(UPSTREAM_EXT)]
    )
    env["FERROCAD_ISOLATION_PYTHON_DIR"] = str(PYTHON_DIR)
    # The stub is the default; make sure a developer's shell does not opt into
    # real Qt for this guard.
    env.pop("FERROCAD_PYSIDE_BACKEND", None)
    return env


def _extension_built() -> bool:
    return any(PYTHON_DIR.glob("ferrocad*.so")) or any(
        PYTHON_DIR.glob("ferrocad*.pyd")
    )


@unittest.skipUnless(
    DRAFT.is_dir() and UPSTREAM_EXT.is_dir(),
    f"upstream Draft workbench is not available at {UPSTREAM}",
)
class QtIsolationTests(unittest.TestCase):
    """D1/D3 plus Q0a's "no real Qt" property, each in a fresh process."""

    @classmethod
    def setUpClass(cls):
        if not _extension_built():
            raise unittest.SkipTest(
                f"the ferrocad extension is not built in {PYTHON_DIR}"
            )

    def run_probe(self, mode: str) -> dict:
        process = subprocess.run(
            [sys.executable, "-c", _PROBE, mode],
            cwd=str(REPO),
            env=_probe_env(),
            capture_output=True,
            text=True,
        )
        self.assertEqual(
            process.returncode,
            0,
            f"probe {mode!r} failed:\nstdout:\n{process.stdout}\nstderr:\n{process.stderr}",
        )
        for line in process.stdout.splitlines():
            if line.startswith(_RESULT_PREFIX):
                return json.loads(line[len(_RESULT_PREFIX) :])
        self.fail(f"probe {mode!r} printed no result:\n{process.stdout}")

    def test_d1_facade_imports_with_every_pyside_name_blocked(self):
        payload = self.run_probe("facade")
        self.assertEqual(payload["gui_up"], 0)
        offenders = [
            name
            for name in payload["modules"]
            if name.split(".")[0] in BLOCKED
        ]
        self.assertEqual(
            offenders,
            [],
            f"headless facade imported a blocked Qt/PySide module: {offenders}",
        )

    def test_d3_params_imports_with_real_qt_blocked(self):
        payload = self.run_probe("draft")
        self.assertEqual(payload["gui_up"], 0)
        draft = payload["draft"]
        self.assertEqual(draft["entries"], 160)
        self.assertEqual(draft["textheight"], 3.5)
        self.assertTrue(draft["shim"], "draftutils imported something other than the stub")
        self.assertTrue(
            draft["origin"].startswith(str(PYTHON_DIR / "PySide")),
            f"the admitted PySide came from {draft['origin']}",
        )

        # The local shim is allowed by name; nothing else may appear.
        offenders = [
            name
            for name in payload["modules"]
            if name.split(".")[0] in BLOCKED and name.split(".")[0] != "PySide"
        ]
        self.assertEqual(
            offenders, [], f"importing draftutils loaded real Qt: {offenders}"
        )
        # ... and the shim itself contains no alias of a real module.
        self.assertIn("PySide.QtCore", payload["modules"])
        self.assertIn("PySide._qt_stub", payload["modules"])

    def test_importing_the_shim_does_not_load_the_installed_pyside6(self):
        payload = self.run_probe("shim-unblocked")
        self.assertTrue(payload["shim"])
        offenders = [
            name
            for name in payload["modules"]
            if name.split(".")[0] in {"PySide6", "PySide2", "shiboken", "shiboken2", "shiboken6"}
        ]
        self.assertEqual(
            offenders,
            [],
            "the pure-Python shim imported a real Qt backend: "
            f"{offenders} (it must be opt-in via FERROCAD_PYSIDE_BACKEND)",
        )


@unittest.skipUnless(
    DRAFT.is_dir(),
    f"upstream Draft workbench is not available at {UPSTREAM}",
)
class DraftContractDriftTests(unittest.TestCase):
    """Q0d: the pinned .ui/contract counts are derivable and checkable without Qt."""

    def run_tool(self, *args: str, blocked: bool = True) -> subprocess.CompletedProcess:
        if blocked:
            command = [sys.executable, "-c", _TOOL_WRAPPER, str(TOOL), *args]
        else:
            command = [sys.executable, str(TOOL), *args]
        env = dict(os.environ)
        env.pop("FERROCAD_PYSIDE_BACKEND", None)
        return subprocess.run(
            command,
            cwd=str(REPO),
            env=env,
            capture_output=True,
            text=True,
        )

    def test_the_committed_contract_does_not_drift(self):
        self.assertTrue(CONTRACT.is_file(), f"missing {CONTRACT}")
        process = self.run_tool("--check", "--root", str(UPSTREAM))
        self.assertEqual(
            process.returncode,
            0,
            f"contract drift:\nstdout:\n{process.stdout}\nstderr:\n{process.stderr}",
        )
        self.assertIn("no drift", process.stdout)
        self.assertIn("19 files", process.stdout)
        self.assertIn("495 widgets", process.stdout)
        self.assertIn("72 contract pairs", process.stdout)

    def test_a_changed_count_reports_the_delta(self):
        pinned = json.loads(CONTRACT.read_text(encoding="utf-8"))
        pinned["ui"]["widget_count"] += 1
        with tempfile.TemporaryDirectory() as tmp:
            drifted = Path(tmp) / "draft-forms.json"
            drifted.write_text(json.dumps(pinned, indent=2, sort_keys=True), encoding="utf-8")
            process = self.run_tool(
                "--check", "--root", str(UPSTREAM), "--contract", str(drifted)
            )
        self.assertEqual(process.returncode, 1, process.stdout + process.stderr)
        self.assertIn("ui.widget_count", process.stdout)
        self.assertIn("pinned 496, found 495", process.stdout)
        self.assertIn("delta -1", process.stdout)

    def test_write_reproduces_the_committed_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            regenerated = Path(tmp) / "draft-forms.json"
            process = self.run_tool(
                "--write", "--root", str(UPSTREAM), "--contract", str(regenerated)
            )
            self.assertEqual(process.returncode, 0, process.stdout + process.stderr)
            self.assertEqual(
                regenerated.read_text(encoding="utf-8"),
                CONTRACT.read_text(encoding="utf-8"),
                "contracts/draft-forms.json is stale; run "
                "`python3 tools/draft_contract.py --write`",
            )


if __name__ == "__main__":
    unittest.main()
