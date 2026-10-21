#!/usr/bin/env python3
"""Assemble (and optionally build) the mdBook edition of the user documentation.

The book is a view over the generated docs, not a second copy of them:

* the pages come from ``docs/user/`` (tutorials, how-to, explanation) and
  ``docs/python-api/`` (reference), unchanged;
* the chapter order comes from the Diátaxis sections;
* links that leave the book are rewritten to the repository on GitHub, so a
  reader who clicks *conformance* lands on the real file instead of a 404.

Because the pages themselves are generated and checked in CI
(``tools/userdocs.py --check``, ``tools/api_docs.py --check``), the book inherits
the same guarantee: it cannot describe behaviour the code no longer has.

Usage::

    python3 tools/build_book.py --check    # assemble to a temp dir and validate
    python3 tools/build_book.py            # assemble into target/user-book/
    python3 tools/build_book.py --build    # also run `mdbook build`

Requires no mdBook for ``--check``; ``--build`` needs ``mdbook`` on ``PATH``.
"""

from __future__ import annotations

import argparse
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parents[1]
DOCS = REPO / "docs"
USER = DOCS / "user"
API = DOCS / "python-api"
DEFAULT_OUT = REPO / "target" / "user-book"
GITHUB = "https://github.com/ferrocad/ferrocad/blob/main"
BOOK_TITLE = "FerroCAD User Guide"

#: (folder under `src/`, part title), in Diátaxis reading order.
PARTS = [
    ("user/tutorials", "Tutorials"),
    ("user/how-to", "How-to guides"),
    ("python-api", "Reference"),
    ("user/explanation", "Explanation"),
]

BOOK_TOML = f"""[book]
title = "{BOOK_TITLE}"
authors = ["The FerroCAD authors"]
language = "en"
src = "src"

[build]
build-dir = "book"
create-missing = false

[output.html]
default-theme = "navy"
"""

LINK = re.compile(r"(!?\[([^\]]*)\]\(([^)\s]+)\))")
HEADING = re.compile(r"^# (.+)$", re.M)


# ---------------------------------------------------------------------------
# Sources
# ---------------------------------------------------------------------------
def book_sources() -> list[tuple[Path, Path]]:
    """(absolute source, path under ``src/``) for every page and asset."""
    pairs: list[tuple[Path, Path]] = []
    for path in sorted(USER.rglob("*")):
        if path.is_file() and (path.suffix == ".md" or "images" in path.parts):
            pairs.append((path, path.relative_to(DOCS)))
    for path in sorted(API.glob("*.md")):
        pairs.append((path, path.relative_to(DOCS)))
    return pairs


def _inside_book(path: Path) -> bool:
    resolved = path.resolve()
    return resolved.is_relative_to(USER.resolve()) or resolved.is_relative_to(
        API.resolve()
    )


def _title(path: Path) -> str:
    match = HEADING.search(path.read_text())
    title = match.group(1).strip() if match else path.stem
    return title.strip("`")


def _order(path: Path) -> tuple:
    """Generated pages follow their workflow's numeric prefix; authored ones trail."""
    match = re.search(r"from tools/workflows/(\d+)_", path.read_text())
    if match:
        return (0, match.group(1), path.name)
    return (1, path.name, "")


# ---------------------------------------------------------------------------
# Link rewriting
# ---------------------------------------------------------------------------
def _rewrite_links(text: str, source: Path) -> str:
    """Point links that leave the book at the repository on GitHub."""
    source_dir = source.parent

    def replace(match: re.Match) -> str:
        whole, label, target = match.group(1), match.group(2), match.group(3)
        if target.startswith(("http://", "https://", "mailto:", "#")):
            return whole
        path, sep, fragment = target.partition("#")
        if not path:
            return whole

        candidate = Path(os.path.normpath(source_dir / path))
        # A link to a directory becomes a link to its index, which mdBook can serve.
        if candidate.is_dir() and _inside_book(candidate / "README.md"):
            candidate = candidate / "README.md"

        if _inside_book(candidate):
            # Normalised so a directory link becomes its index page, which mdBook serves.
            keep = os.path.relpath(candidate, source_dir).replace(os.sep, "/")
            new_label = label
        else:
            try:
                repo_relative = candidate.resolve().relative_to(REPO)
            except ValueError:
                return whole  # outside the repository: leave it to the validator
            keep = f"{GITHUB}/{repo_relative.as_posix()}"
            # Pages use the path as the label; point that at where the link now goes.
            if label == target or label.strip("`") == target:
                new_label = repo_relative.as_posix()
                if label.startswith("`"):
                    new_label = f"`{new_label}`"
            else:
                new_label = label
        if sep:
            keep += "#" + fragment
        return f"{'!' if whole.startswith('!') else ''}[{new_label}]({keep})"

    return LINK.sub(replace, text)


# ---------------------------------------------------------------------------
# Assembly
# ---------------------------------------------------------------------------
def _summary(src: Path) -> str:
    lines = ["# Summary", "", "[Introduction](user/README.md)", ""]

    for folder, part in PARTS:
        pages = [
            rel
            for _source, rel in _copied(src)
            if rel.as_posix().startswith(folder + "/")
            and rel.name != "README.md"
        ]
        if not pages:
            continue
        lines += [f"# {part}", ""]
        if folder == "python-api":
            lines.append("- [Python API reference](python-api/README.md)")
            for rel in sorted(pages, key=lambda r: r.as_posix()):
                lines.append(f"  - [{_title(src / rel)}]({rel.as_posix()})")
        else:
            for rel in sorted(pages, key=lambda r: _order(src / r)):
                lines.append(f"- [{_title(src / rel)}]({rel.as_posix()})")
        lines.append("")

    return "\n".join(lines).rstrip() + "\n"


def _copied(src: Path) -> list[tuple[Path, Path]]:
    return [
        (src / rel, rel)
        for rel in sorted(p.relative_to(src) for p in src.rglob("*") if p.is_file())
    ]


def assemble(out: Path) -> Path:
    """Write a complete mdBook source tree under ``out``; return ``out/src``."""
    src = out / "src"
    if src.exists():
        shutil.rmtree(src)
    src.mkdir(parents=True)

    for source, rel in book_sources():
        target = src / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        if source.suffix == ".md":
            target.write_text(_rewrite_links(source.read_text(), source))
        else:
            shutil.copyfile(source, target)

    (src / "SUMMARY.md").write_text(_summary(src))
    (out / "book.toml").write_text(BOOK_TOML)
    return src


# ---------------------------------------------------------------------------
# Validation
# ---------------------------------------------------------------------------
def validate(src: Path) -> list[str]:
    """Structural checks that need no mdBook."""
    problems: list[str] = []
    summary = (src / "SUMMARY.md").read_text()
    listed = {m.group(1) for m in re.finditer(r"\]\(([^)]+\.md)\)", summary)}

    for path in sorted(p for p in src.rglob("*.md") if p.name != "SUMMARY.md"):
        rel = path.relative_to(src).as_posix()
        if rel not in listed:
            problems.append(f"not in SUMMARY: {rel}")
        for match in re.finditer(r"\]\(([^)]+)\)", path.read_text()):
            target = match.group(1)
            if target.startswith(("http://", "https://", "mailto:", "#")):
                continue
            target = target.split("#")[0]
            if not target:
                continue
            resolved = Path(os.path.normpath(path.parent / target))
            if not resolved.exists():
                problems.append(f"{rel} -> {target} (missing)")

    for rel in sorted(listed):
        if not (src / rel).is_file():
            problems.append(f"SUMMARY lists a missing page: {rel}")

    return problems


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------
def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="validate, write nothing")
    parser.add_argument("--build", action="store_true", help="run `mdbook build`")
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT, help="output directory")
    args = parser.parse_args(argv)

    if args.check:
        out = Path(tempfile.mkdtemp(prefix="ferrocad-book-"))
    else:
        out = args.out
        out.mkdir(parents=True, exist_ok=True)

    src = assemble(out)
    problems = validate(src)
    if problems:
        for problem in problems:
            print(f"error: {problem}", file=sys.stderr)
        return 1

    chapters = len(
        [
            p
            for p in src.rglob("*.md")
            if p.name not in ("SUMMARY.md",)
        ]
    )

    if args.check:
        shutil.rmtree(out, ignore_errors=True)
        print(f"book is consistent ({chapters} chapters)")
        return 0

    print(f"assembled {chapters} chapters into {out.relative_to(REPO)}")
    if not args.build:
        print("run with --build to render it with mdbook")
        return 0

    if shutil.which("mdbook") is None:
        print(
            "error: mdbook is not on PATH; install it with "
            "`cargo install mdbook` (or `cargo binstall mdbook`)",
            file=sys.stderr,
        )
        return 1

    result = subprocess.run(
        ["mdbook", "build", str(out)], cwd=REPO, capture_output=True, text=True
    )
    if result.returncode != 0:
        print(result.stdout, file=sys.stderr)
        print(result.stderr, file=sys.stderr)
        return result.returncode
    index = out / "book" / "index.html"
    print(f"built {index.relative_to(REPO)}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
