"""A ``PySide`` compatibility package over the installed PySide6.

Upstream FreeCAD's Qt5 workbenches (and Draft, at import time) write
``import PySide.QtCore as QtCore`` and ``from PySide.QtCore import
QT_TRANSLATE_NOOP``. The system ships PySide6, so this package aliases the Qt6
modules into ``sys.modules`` under the Qt5 names. Importing them here means
``PySide6.QtCore`` is imported exactly once: both names refer to the same module
object.

Only the modules needed for *imports* are aliased. Qt5-only names (for example
``QtCore.SIGNAL``) are not emulated; the GUI work will meet those individually.

Missing Qt modules are skipped rather than fatal: ``QtSvg`` is not installed on
every system, and its absence must not break ``import PySide.QtCore``.
"""

from __future__ import annotations

import importlib
import sys

_PYSIDE6 = "PySide6"

# Import order matters only for readability; QtCore comes first because the
# package-level ``QT_TRANSLATE_NOOP`` comes from it.
_ALIASED_MODULES = ("QtCore", "QtGui", "QtWidgets", "QtSvg")

# The subset that was actually installed; `__all__` must not name an absent one.
_INSTALLED = []


def _install_aliases():
    for name in _ALIASED_MODULES:
        try:
            module = importlib.import_module(f"{_PYSIDE6}.{name}")
        except ImportError:
            # The module is genuinely absent (QtSvg on this system). A later
            # ``import PySide.QtSvg`` will raise ModuleNotFoundError as usual.
            continue
        sys.modules[f"{__name__}.{name}"] = module
        globals()[name] = module
        _INSTALLED.append(name)


_install_aliases()


def _translate_noop(context, text):
    """Qt5's ``QT_TRANSLATE_NOOP``: mark a string for translation, return it."""
    return text


# ``QT_TRANSLATE_NOOP`` lives in ``QtCore``. Some Qt6 builds drop the bare
# module-level spelling, so fall back to our own no-op rather than failing the
# whole package import.
_qtcore = sys.modules.get(f"{__name__}.QtCore")
QT_TRANSLATE_NOOP = getattr(_qtcore, "QT_TRANSLATE_NOOP", _translate_noop)
del _qtcore

__all__ = ["QT_TRANSLATE_NOOP", *_INSTALLED]
