"""``FreeCAD.Console`` — minimal logging facade.

Upstream routes console output through ``FreeCAD.Console.Print*``. This POC
writes to stderr; it is enough for upstream tests that log progress messages
(e.g. `UnicodeTests.py`). The status/verbosity machinery is not implemented.
"""

from __future__ import annotations

import sys


def _write(text: str) -> None:
    sys.stderr.write(text)
    sys.stderr.flush()


def PrintLog(text: str = "", end: str = "\n") -> None:
    _write(text + end)


def PrintMessage(text: str = "", end: str = "\n") -> None:
    _write(text + end)


def PrintWarning(text: str = "", end: str = "\n") -> None:
    _write(text + end)


def PrintError(text: str = "", end: str = "\n") -> None:
    _write(text + end)


def PrintStatus(*args, **kwargs) -> None:
    """Compatibility stub; upstream prints an optional status/type line."""
    if args:
        _write(str(args[0]) + "\n")


__all__ = ["PrintLog", "PrintMessage", "PrintWarning", "PrintError", "PrintStatus"]
