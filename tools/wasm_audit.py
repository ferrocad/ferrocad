#!/usr/bin/env python3
"""Audit wasm-reachable crates for APIs that compile for wasm but panic at run time.

``cargo check --target wasm32-unknown-unknown`` cannot catch this class: the calls
type-check, and the wasm backend of ``std`` panics (or aborts) only when they run. Two
of these reached the browser before this check existed — ``std::time::Instant::now()``
in the frame timer and again in the stage sync, both panicking with "time not
implemented on this platform" on the first frame.

Flagged, because each panics on ``wasm32-unknown-unknown``:

=====================  ==========================================================
``std::time``          ``Instant``/``SystemTime``: "time not implemented on this
                       platform". Use ``web_time`` (a ``std`` alias on native).
``std::thread``        ``spawn``/``Builder``: no threads; ``sleep`` has no clock.
=====================  ==========================================================

Not flagged: ``std::fs`` and ``std::net`` return ``Err(Unsupported)`` there, and the
hosts behind them already take their bytes from the platform. ``Duration`` is pure
arithmetic and fine.

Scope: the workspace crates in the dependency closure of the wasm entry points below,
scanning their ``src/``. Test code is excluded twice over — ``tests/`` directories by
path, and inline ``#[cfg(test)] mod`` blocks by their convention of running to the end
of the file. Third-party crates are not audited. A file that no wasm target compiles
says so on a line of its own::

    // wasm-audit: native-only — declared behind #[cfg(not(target_family = "wasm"))]

Usage::

    python3 tools/wasm_audit.py            # report findings
    python3 tools/wasm_audit.py --check    # same, for a CI line (exit 1 on findings)
    python3 tools/wasm_audit.py --list     # the crates in scope

Exits 2 when the graph cannot be read (no cargo, or a stale ``Cargo.lock``), because an
audit that silently checked nothing is worse than none.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]

# The crates a wasm build starts from. Everything they reach is compiled to wasm, so
# everything they reach is in scope.
ENTRIES = ("ferrocad_web_gpui", "ferrocad_shell_pywasm")

# A file that no wasm target compiles opts out of the scan with this line, and says why.
OPT_OUT = re.compile(r"^\s*//\s*wasm-audit:\s*native-only\b")

# `#[cfg(test)]` immediately before a `mod`, which is where a test module starts.
CFG_TEST = re.compile(r"^\s*#\[cfg\(test\)\]")
MOD_DECL = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+")

# `use std::time::Instant;` and the brace form `use std::time::{Duration, Instant};`
STD_TIME_IMPORT = re.compile(r"\buse\s+std::time::(?:\{[^}]*\bInstant\b[^}]*\}|Instant\b)")
STD_TIME_PATH = re.compile(r"\bstd::time::(?:Instant|SystemTime)\b")
INSTANT_PATH = re.compile(r"\bInstant::")
STD_THREAD_IMPORT = re.compile(r"\buse\s+std::thread\b")
THREAD_PATH = re.compile(r"\b(?:std::)?thread::(?:spawn|sleep|Builder)\b")

REASONS = {
    "std::time": 'panics: "time not implemented on this platform"',
    "std::thread": "panics: this target has no threads",
}


# Path components that hold code no wasm build compiles, wherever they sit under `src/`.
SKIP_COMPONENTS = frozenset({"tests", "examples", "benches"})


def strip_comment(line: str) -> str:
    """`line` without its trailing ``//`` comment.

    Whole-line comments are dropped entirely, so prose that *names* these APIs (this
    module's own docstrings included) is not a finding. Quotes are tracked so a ``//``
    inside a string literal survives; a bare lifetime can start that tracking and hide
    a comment, which costs a false positive rather than a false negative.
    """
    quote = None
    index = 0
    while index < len(line) - 1:
        char = line[index]
        if quote is not None:
            if char == "\\":
                index += 2
                continue
            if char == quote:
                quote = None
        elif char in "\"'":
            quote = char
        elif char == "/" and line[index + 1] == "/":
            return line[:index]
        index += 1
    return line


def scan_source(text: str) -> list[tuple[int, str, str]]:
    """Findings in one file, as ``(line number, symbol, the line)``.

    One finding per symbol per file: a file that uses ``Instant`` in twenty places has
    one cause, and a wall of duplicates hides the others.

    A ``#[cfg(test)] mod`` ends the scan: a test module is last in a file by
    convention, and test code is not compiled into a wasm build.
    """
    lines = text.splitlines()
    findings: list[tuple[int, str, str]] = []
    seen: set[str] = set()
    std_instant = False
    for index, raw in enumerate(lines):
        if OPT_OUT.match(raw):
            return []
        if CFG_TEST.match(raw) and any(
            MOD_DECL.match(line) for line in lines[index + 1 : index + 4]
        ):
            break
        line = strip_comment(raw)
        if not line.strip():
            continue
        if STD_TIME_IMPORT.search(line):
            std_instant = True
        symbol = None
        if THREAD_PATH.search(line):
            symbol = "std::thread"
        elif STD_TIME_PATH.search(line) or (std_instant and INSTANT_PATH.search(line)):
            symbol = "std::time"
        if symbol is not None and symbol not in seen:
            seen.add(symbol)
            findings.append((index + 1, symbol, line.strip()))
    return findings


def closure(entries) -> tuple[list[dict], str]:
    """The workspace packages reachable from `entries`, and why it failed."""
    command = [
        "cargo",
        "metadata",
        "--format-version",
        "1",
        "--locked",
        "--manifest-path",
        str(REPO / "Cargo.toml"),
    ]
    try:
        result = subprocess.run(
            command, capture_output=True, text=True, cwd=REPO, check=False
        )
    except FileNotFoundError:
        return [], "cargo is not on PATH"
    if result.returncode != 0:
        detail = (result.stderr or result.stdout).strip().splitlines()
        return [], detail[-1] if detail else "cargo metadata failed"

    data = json.loads(result.stdout)
    packages = {package["id"]: package for package in data["packages"]}
    # Normal dependencies only: `cargo metadata` also reports dev- and build-deps by
    # default, and a dev-dependency on a native host would drag the whole desktop stack
    # into an audit of what a wasm build compiles. Target-specific normal deps stay in:
    # the closure is then a superset, and a native-only module marks itself out.
    edges = {}
    for node in data["resolve"]["nodes"]:
        edges[node["id"]] = [
            dep["pkg"]
            for dep in node.get("deps") or []
            if any(kind.get("kind") is None for kind in dep.get("dep_kinds") or [{}])
        ]

    by_name = {}
    for member in data["workspace_members"]:
        by_name.setdefault(packages[member]["name"], member)

    pending = [by_name[name] for name in entries if name in by_name]
    missing = [name for name in entries if name not in by_name]
    if missing:
        return [], f"no workspace package named {', '.join(missing)}"

    reached: dict[str, dict] = {}
    while pending:
        current = pending.pop()
        if current in reached:
            continue
        reached[current] = packages[current]
        pending.extend(edges.get(current, []))

    members = [package for pid, package in reached.items() if pid in set(data["workspace_members"])]
    members.sort(key=lambda package: package["name"])
    return members, ""


def sources(package: dict) -> list[Path]:
    """The Rust sources a wasm build compiles for `package`.

    `tests/` (and `examples/`, `benches/`) live outside `src/`, but a crate can keep
    its test modules under `src/…/tests/`, so a `tests` component is excluded wherever
    it sits.
    """
    root = Path(package["manifest_path"]).parent / "src"
    return sorted(
        path
        for path in root.rglob("*.rs")
        if path.is_file() and not SKIP_COMPONENTS.intersection(path.parts)
    )


def audit(entries) -> tuple[list[dict], list[tuple[Path, int, str, str]], str]:
    """The packages in scope, the findings, and why it could not run."""
    members, error = closure(entries)
    if error:
        return [], [], error
    findings: list[tuple[Path, int, str, str]] = []
    for package in members:
        for path in sources(package):
            text = path.read_text(encoding="utf-8", errors="replace")
            for number, symbol, source in scan_source(text):
                findings.append((path, number, symbol, source))
    return members, findings, ""


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description="Audit wasm-reachable crates for wasm-panicking APIs.")
    parser.add_argument("--check", action="store_true", help="exit 1 on findings (the CI form; this tool never writes)")
    parser.add_argument("--list", action="store_true", help="print the crates in scope and exit")
    parser.add_argument(
        "--entries",
        nargs="+",
        default=list(ENTRIES),
        metavar="CRATE",
        help=f"the wasm entry crates (default: {' '.join(ENTRIES)})",
    )
    args = parser.parse_args(argv)

    members, findings, error = audit(args.entries)
    if error:
        print(f"wasm audit: cannot read the dependency graph: {error}", file=sys.stderr)
        print("wasm audit: run `cargo metadata --locked` to see why", file=sys.stderr)
        return 2

    if args.list:
        for package in members:
            print(package["name"])
        return 0

    if findings:
        print(
            f"wasm audit: {len(findings)} finding(s) in {len({f[0] for f in findings})} file(s) "
            f"— {len(members)} crates in scope"
        )
        for path, line_number, symbol, source in findings:
            relative = path.relative_to(REPO)
            print(f"  {relative}:{line_number}  {symbol}  {REASONS[symbol]}")
            print(f"      {source}")
        print("Use `web_time` (a `std` alias on native) and, where a file is native-only,")
        print("mark it with `// wasm-audit: native-only — <why>`.")
        return 1

    print(
        f"wasm audit: clean — {sum(len(sources(p)) for p in members)} source files across "
        f"{len(members)} crates reachable from {' '.join(args.entries)}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
