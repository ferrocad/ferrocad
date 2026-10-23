#!/usr/bin/env python3
"""Q0d: pin the upstream Draft form inventory without needing Qt.

The upstream Draft workbench ships 19 ``.ui`` files and ten Python modules that
drive them. Neither the file inventory nor the widget/member contract needs a Qt
runtime to measure: the forms are XML and the call sites are plain Python. This
tool derives both with the standard library and compares them with
``contracts/draft-forms.json``, printing a per-number delta when upstream moves.

What is derived:

* ``ui.file_count`` / ``ui.widget_count`` / ``ui.class_count`` and the full
  class inventory, from ``xml.etree`` over ``Resources/ui/*.ui``;
* ``ui.plain_class_count`` / ``ui.custom_class_count`` from the ``Gui::``
  prefix on the class name;
* per-file widget and class counts;
* the panel contract: distinct ``(widget class, member)`` pairs used by the ten
  ``.ui``-backed modules (``self.form.<widget>.<member>`` and
  ``self.dialog.<widget>.<member>``), plus per-form counts and root signals.

What is *not* derivable without running Qt is listed in ``NOTES`` and printed
with the report. Counts that this tool cannot reproduce are not pinned; the
measured ones are.

Usage::

    python3 tools/draft_contract.py                     # derive + report
    python3 tools/draft_contract.py --check             # fail on drift (CI)
    python3 tools/draft_contract.py --write             # re-pin after a bump
    python3 tools/draft_contract.py --json              # machine-readable report
    python3 tools/draft_contract.py --root ../freecad-upstream
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import xml.etree.ElementTree as ET
from collections import Counter
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_ROOT = Path(os.environ.get("FERROCAD_UPSTREAM", REPO.parent / "freecad-upstream"))
DEFAULT_CONTRACT = REPO / "contracts" / "draft-forms.json"

#: The upstream revision these counts were first measured on. The tool does not
#: verify the checkout's git revision (it must run without git); the counts are
#: the check. Bump this string with ``--write`` when the tree moves.
UPSTREAM_REVISION = "33adf61"

#: The ten ``.ui``-backed modules and the form each loads. ``task_scale.py`` is
#: excluded: it composes its widgets in code and has no ``.ui`` file
#: (``qt-strategy.md`` §2.5).
FORMS = {
    "drafttaskpanels/task_orthoarray.py": "TaskPanel_OrthoArray.ui",
    "drafttaskpanels/task_polararray.py": "TaskPanel_PolarArray.ui",
    "drafttaskpanels/task_circulararray.py": "TaskPanel_CircularArray.ui",
    "drafttaskpanels/task_shapestring.py": "TaskShapeString.ui",
    "drafttaskpanels/task_selectplane.py": "TaskSelectPlane.ui",
    "draftguitools/gui_setstyle.py": "TaskPanel_SetStyle.ui",
    "draftguitools/gui_hatch.py": "dialogHatch.ui",
    "draftguitools/gui_layers.py": "dialogLayers.ui",
    "draftguitools/gui_annotationstyleeditor.py": "dialog_AnnotationStyleEditor.ui",
    "DxfImportDialog.py": "preferences-dxf-import.ui",
}

#: ``self.form.<widget>.<member>`` / ``self.dialog.<widget>.<member>``. The
#: optional trailing ``.connect`` marks a signal use (a heuristic: a slot is
#: called through it, and §2.4 counts signals separately).
_ACCESS = re.compile(r"self\.(?:form|dialog)\.(\w+)\.(\w+)(\.connect)?")

#: The form root's own signals, e.g. ``self.dialog.rejected.connect(...)``.
_ROOT_SIGNALS = {"accepted", "rejected"}

NOTES = [
    "Counts are derived with the standard library only: xml.etree for the "
    "forms, a text pass for the Python call sites. No Qt, no display.",
    "The class inventory, per-file widget counts and the 72 distinct "
    "(class, member) pairs match qt-strategy.md §2.3–§2.4.",
    "'accesses' pins every textual access this tool finds; it is not "
    "§2.4's 'Calls' column, which uses a different counting rule.",
    "Method-vs-signal is inferred from a trailing .connect, a heuristic; the "
    "signal payloads and connection counts need the panels to run and are "
    "not pinned.",
    "Not reproducible without Qt: widget construction, signal payloads, "
    "alias-based access (none was found in these modules), and §2.4's "
    "'7 signals / 34 connections' aggregate for the six core panels.",
    "The checkout's git revision is not verified; counts are the drift check. "
    f"Declared revision: {UPSTREAM_REVISION}.",
]


# ---------------------------------------------------------------------------
# Derivation
# ---------------------------------------------------------------------------


def find_draft(root: Path) -> Path:
    """Return the ``src/Mod/Draft`` directory under ``root``."""
    root = Path(root)
    for candidate in (root / "src" / "Mod" / "Draft", root / "Mod" / "Draft", root):
        if (candidate / "Resources" / "ui").is_dir():
            return candidate
    raise SystemExit(
        f"draft contract: no Draft workbench under {root} "
        "(expected src/Mod/Draft/Resources/ui or a path to the Draft directory)"
    )


def derive_ui(draft: Path) -> dict:
    ui_dir = draft / "Resources" / "ui"
    files = sorted(ui_dir.glob("*.ui"))
    classes: Counter = Counter()
    per_file: dict[str, dict] = {}
    widget_total = 0

    for path in files:
        try:
            root = ET.parse(path).getroot()
        except ET.ParseError as error:
            raise SystemExit(f"draft contract: cannot parse {path}: {error}") from error
        widgets = list(root.iter("widget"))
        counts = Counter(w.get("class") for w in widgets if w.get("class"))
        widget_total += len(widgets)
        classes.update(counts)
        per_file[path.name] = {
            "widgets": len(widgets),
            "classes": dict(sorted(counts.items())),
        }

    plain = sum(n for name, n in classes.items() if not name.startswith("Gui::"))
    custom = sum(n for name, n in classes.items() if name.startswith("Gui::"))
    plain_classes = sum(1 for name in classes if not name.startswith("Gui::"))
    custom_classes = sum(1 for name in classes if name.startswith("Gui::"))

    return {
        "file_count": len(files),
        "widget_count": widget_total,
        "class_count": len(classes),
        "plain_class_count": plain_classes,
        "custom_class_count": custom_classes,
        "plain_instance_count": plain,
        "custom_instance_count": custom,
        "classes": dict(sorted(classes.items())),
        "files": per_file,
    }


def derive_contract(draft: Path, ui: dict) -> dict:
    forms: dict[str, dict] = {}
    all_pairs: set[tuple[str, str]] = set()
    unknown: dict[str, list[str]] = {}

    for module, ui_name in sorted(FORMS.items()):
        path = draft / module
        if not path.is_file():
            raise SystemExit(f"draft contract: missing panel module {path}")
        if ui_name not in ui["files"]:
            raise SystemExit(f"draft contract: {ui_name} is not in Resources/ui")

        widgets = {
            name: cls
            for name, cls in _widget_names(draft / "Resources" / "ui" / ui_name).items()
        }
        pairs: Counter = Counter()
        root_signals: Counter = Counter()
        unknown_names: Counter = Counter()

        for match in _ACCESS.finditer(path.read_text(encoding="utf-8")):
            widget, member, connected = match.groups()
            if widget in _ROOT_SIGNALS and member == "connect":
                root_signals[widget] += 1
                continue
            cls = widgets.get(widget)
            if cls is None:
                unknown_names[widget] += 1
                continue
            pairs[(cls, member)] += 1
            all_pairs.add((cls, member))

        if unknown_names:
            unknown[module] = sorted(unknown_names)
        forms[module] = {
            "ui": ui_name,
            "widgets": ui["files"][ui_name]["widgets"],
            "pair_count": len(pairs),
            "accesses": sum(pairs.values()),
            "pairs": [list(pair) for pair in sorted(pairs)],
            "root_signals": sorted(root_signals),
        }

    return {
        "pair_count": len(all_pairs),
        "pairs": [list(pair) for pair in sorted(all_pairs)],
        "forms": forms,
        "unknown_widget_names": unknown,
    }


def _widget_names(path: Path) -> dict[str, str]:
    root = ET.parse(path).getroot()
    return {
        widget.get("name"): widget.get("class")
        for widget in root.iter("widget")
        if widget.get("name") and widget.get("class")
    }


def derive(root: Path) -> dict:
    draft = find_draft(root)
    ui = derive_ui(draft)
    return {
        "schema": 1,
        "measured": "2026-10-23",
        "upstream": {
            "revision": UPSTREAM_REVISION,
            "path": "src/Mod/Draft",
        },
        "ui": ui,
        "contract": derive_contract(draft, ui),
        "notes": NOTES,
    }


# ---------------------------------------------------------------------------
# Comparison
# ---------------------------------------------------------------------------


def _delta(pinned: int, found: int) -> str:
    if isinstance(pinned, int) and isinstance(found, int) and pinned != found:
        return f" (delta {found - pinned:+d})"
    return ""


def compare(pinned: dict, found: dict) -> list[str]:
    problems: list[str] = []

    def scalar(path: str, expected, actual) -> None:
        if expected != actual:
            problems.append(
                f"{path}: pinned {expected!r}, found {actual!r}"
                f"{_delta(expected, actual)}"
            )

    def counts(path: str, expected: dict, actual: dict) -> None:
        for key in sorted(set(expected) | set(actual)):
            if expected.get(key) != actual.get(key):
                problems.append(
                    f"{path}.{key}: pinned {expected.get(key)!r}, "
                    f"found {actual.get(key)!r}"
                    f"{_delta(expected.get(key), actual.get(key))}"
                )

    def pair_set(path: str, expected, actual) -> None:
        expected_set = {tuple(pair) for pair in expected}
        actual_set = {tuple(pair) for pair in actual}
        for pair in sorted(expected_set - actual_set):
            problems.append(f"{path}: missing pair {pair[0]}.{pair[1]}")
        for pair in sorted(actual_set - expected_set):
            problems.append(f"{path}: new pair {pair[0]}.{pair[1]}")

    pinned_ui = pinned.get("ui", {})
    found_ui = found.get("ui", {})
    for key in (
        "file_count",
        "widget_count",
        "class_count",
        "plain_class_count",
        "custom_class_count",
        "plain_instance_count",
        "custom_instance_count",
    ):
        scalar(f"ui.{key}", pinned_ui.get(key), found_ui.get(key))
    counts("ui.classes", pinned_ui.get("classes", {}), found_ui.get("classes", {}))

    pinned_files = pinned_ui.get("files", {})
    found_files = found_ui.get("files", {})
    for name in sorted(set(pinned_files) | set(found_files)):
        if name not in found_files:
            problems.append(f"ui.files.{name}: pinned but missing")
            continue
        if name not in pinned_files:
            problems.append(
                f"ui.files.{name}: new file with "
                f"{found_files[name].get('widgets')} widgets"
            )
            continue
        scalar(
            f"ui.files.{name}.widgets",
            pinned_files[name].get("widgets"),
            found_files[name].get("widgets"),
        )
        counts(
            f"ui.files.{name}.classes",
            pinned_files[name].get("classes", {}),
            found_files[name].get("classes", {}),
        )

    pinned_contract = pinned.get("contract", {})
    found_contract = found.get("contract", {})
    scalar(
        "contract.pair_count",
        pinned_contract.get("pair_count"),
        found_contract.get("pair_count"),
    )
    pair_set(
        "contract.pairs",
        pinned_contract.get("pairs", []),
        found_contract.get("pairs", []),
    )
    scalar(
        "contract.unknown_widget_names",
        pinned_contract.get("unknown_widget_names", {}),
        found_contract.get("unknown_widget_names", {}),
    )

    pinned_forms = pinned_contract.get("forms", {})
    found_forms = found_contract.get("forms", {})
    for module in sorted(set(pinned_forms) | set(found_forms)):
        if module not in found_forms:
            problems.append(f"contract.forms.{module}: pinned but missing")
            continue
        if module not in pinned_forms:
            problems.append(f"contract.forms.{module}: new form module")
            continue
        for key in ("ui", "widgets", "pair_count", "accesses"):
            scalar(
                f"contract.forms.{module}.{key}",
                pinned_forms[module].get(key),
                found_forms[module].get(key),
            )
        pair_set(
            f"contract.forms.{module}.pairs",
            pinned_forms[module].get("pairs", []),
            found_forms[module].get("pairs", []),
        )
        scalar(
            f"contract.forms.{module}.root_signals",
            pinned_forms[module].get("root_signals", []),
            found_forms[module].get("root_signals", []),
        )

    return problems


def summary(contract: dict) -> str:
    ui = contract["ui"]
    form = contract["contract"]
    return (
        f"draft contract: {ui['file_count']} files, "
        f"{ui['class_count']} classes "
        f"({ui['plain_class_count']} Qt + {ui['custom_class_count']} Gui::), "
        f"{ui['widget_count']} widgets, "
        f"{form['pair_count']} contract pairs"
    )


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def load_pinned(path: Path) -> dict:
    if not path.is_file():
        raise SystemExit(f"draft contract: pinned file not found: {path}")
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as error:
        raise SystemExit(f"draft contract: {path} is not valid JSON: {error}") from error


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(
        description="Derive and check the upstream Draft form contract without Qt."
    )
    parser.add_argument(
        "--root",
        default=str(DEFAULT_ROOT),
        help=f"freecad-upstream checkout (default: {DEFAULT_ROOT})",
    )
    parser.add_argument(
        "--contract",
        default=str(DEFAULT_CONTRACT),
        help=f"pinned JSON file (default: {DEFAULT_CONTRACT})",
    )
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="fail on drift")
    mode.add_argument("--write", action="store_true", help="re-pin the JSON file")
    parser.add_argument("--json", action="store_true", help="print the derived JSON")
    args = parser.parse_args(argv)

    found = derive(Path(args.root))
    contract_path = Path(args.contract)

    if args.json:
        print(json.dumps(found, indent=2, sort_keys=True))
        return 0

    if args.write:
        contract_path.parent.mkdir(parents=True, exist_ok=True)
        contract_path.write_text(
            json.dumps(found, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        print(f"wrote {contract_path}")
        print(summary(found))
        return 0

    if args.check:
        pinned = load_pinned(contract_path)
        problems = compare(pinned, found)
        if problems:
            print(
                "draft contract drift against "
                f"{contract_path} (declared upstream {UPSTREAM_REVISION}):"
            )
            for problem in problems:
                print(f"  {problem}")
            print(
                "Re-run `python3 tools/draft_contract.py --write` after "
                "confirming the upstream change."
            )
            return 1
        print(summary(found) + " — no drift")
        for note in found["notes"]:
            print(f"  note: {note}")
        return 0

    print(summary(found))
    ui = found["ui"]
    print(f"  source: {find_draft(Path(args.root))}")
    print(f"  classes: {ui['plain_class_count']} plain Qt, "
          f"{ui['custom_class_count']} Gui::")
    forms = found["contract"]["forms"]
    print("  forms:")
    for module in sorted(forms):
        entry = forms[module]
        print(
            f"    {module}: {entry['ui']} — {entry['widgets']} widgets, "
            f"{entry['pair_count']} pairs, {entry['accesses']} accesses"
        )
    print("  not derivable without Qt:")
    for note in found["notes"]:
        print(f"    - {note}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except BrokenPipeError:  # pragma: no cover - piping into head
        sys.exit(0)
