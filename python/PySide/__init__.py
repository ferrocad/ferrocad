"""A pure-Python ``PySide`` compatibility package for upstream FreeCAD scripts.

Upstream FreeCAD's Qt5 workbenches write ``import PySide.QtCore as QtCore``,
``from PySide.QtCore import QT_TRANSLATE_NOOP`` and
``import PySide.QtWidgets as QtWidgets``. FerroCAD ships those scripts as
source, so the import statements cannot be removed. This package supplies the
small name subset they evaluate **at import time**, without requiring any Qt
runtime:

===============  =========================================================
Module           Names
===============  =========================================================
``QtCore``       ``QT_TRANSLATE_NOOP``, ``QCoreApplication``, ``QLocale``,
                 ``QObject``, ``QTimer``, ``SIGNAL``, ``QIODevice``,
                 ``QFile``, ``QFileInfo``, ``QStandardPaths``, plus the
                 ``Qt`` enum namespace (arrow keys and modifiers)
``QtWidgets``    ``QApplication`` and the names upstream uses as class bases
                 (``QWidget``, ``QLineEdit``, ``QStyledItemDelegate``,
                 ``QDialog``, ``QTextEdit``, ``QPlainTextEdit``,
                 ``QAbstractSpinBox``)
``QtGui``        module object only; the headless Draft path evaluates no
                 QtGui name at import time
===============  =========================================================

Every name outside those allowlists raises ``AttributeError`` naming the module
and the attribute. Nothing returns ``None`` to hide a missing feature.

**Resources.** ``QtCore.QFile(":/ui/x.ui")`` reads a mod's loose
``Resources/`` tree (``<root>/Resources/x.ui`` or
``<root>/<Mod>/Resources/x.ui``), with hit *and* miss caching. This is what
lets ``draftutils/params.py`` build ``PARAM_DICT`` from the preference ``.ui``
files without ``pyside-rcc``.

**Real Qt is opt-in.** Set ``FERROCAD_PYSIDE_BACKEND=PySide6`` (or
``PySide2``) in the environment to alias the installed distribution's
``QtCore``/``QtGui``/``QtWidgets`` (and ``QtSvg`` when present) instead of
using the pure-Python stub. This is intended for experiments that need real
widgets; it is never the default, and headless CI must not set it. In that mode
``:/`` resource paths are *not* remapped (Qt's own resource system applies).
An unrecognised value fails the import rather than silently falling back.

The stub lives in :mod:`PySide._qt_stub`; its exports are the allowlists
``QTCORE_EXPORTS``, ``QTGUI_EXPORTS`` and ``QTWIDGETS_EXPORTS``.
"""

from __future__ import annotations

import importlib
import importlib.machinery
import os
import sys

from . import _qt_stub as _stub

#: Set to ``PySide6`` or ``PySide2`` to alias a real Qt installation.
BACKEND_ENV = "FERROCAD_PYSIDE_BACKEND"

_STUB_BACKEND = "stub"
_SUBMODULES = ("QtCore", "QtGui", "QtWidgets")


def _selected_backend() -> str:
    value = os.environ.get(BACKEND_ENV, "").strip()
    return _STUB_BACKEND if not value else value


def _new_stub_module(name: str, exports: dict) -> _stub.QtStubModule:
    module = _stub.QtStubModule(
        f"{__name__}.{name}",
        _STUB_DOCS.get(name, f"Pure-Python stand-in for PySide.{name}."),
    )
    module.__package__ = __name__
    module.__spec__ = importlib.machinery.ModuleSpec(module.__name__, None)
    module.__file__ = _stub.__file__
    module.__all__ = tuple(sorted(exports))
    setattr(module, _stub.STUB_MARKER, True)
    for key, value in exports.items():
        setattr(module, key, value)
    sys.modules[module.__name__] = module
    globals()[name] = module
    return module


_STUB_DOCS = {
    "QtCore": (
        "Pure-Python QtCore subset: translation, timers, objects and loose-file "
        "access for upstream Draft. No Qt runtime is imported."
    ),
    "QtGui": (
        "Pure-Python QtGui stand-in. The headless Draft import path evaluates no "
        "QtGui name, so this module exports none; a missing name raises. GUI "
        "modules (e.g. gui_hyperlink's QDesktopServices) still fail loudly."
    ),
    "QtWidgets": (
        "Pure-Python QtWidgets stand-in: QApplication.translate and the class "
        "bases upstream subclasses at import time (Draft's widgets and "
        "SimpleCAD's QDialog). Widgets are not constructed."
    ),
}


def _install_stub() -> list[str]:
    exports_by_module = {
        "QtCore": _stub.QTCORE_EXPORTS,
        "QtGui": _stub.QTGUI_EXPORTS,
        "QtWidgets": _stub.QTWIDGETS_EXPORTS,
    }
    installed = []
    for name in _SUBMODULES:
        _new_stub_module(name, dict(exports_by_module[name]))
        installed.append(name)
    return installed


def _install_real(distribution: str) -> list[str]:
    installed = []
    for name in (*_SUBMODULES, "QtSvg"):
        try:
            module = importlib.import_module(f"{distribution}.{name}")
        except ImportError:
            if name == "QtSvg":
                # QtSvg is genuinely optional; its absence must not break the
                # rest of the package (upstream Draft only needs it in the GUI).
                continue
            raise
        sys.modules[f"{__name__}.{name}"] = module
        globals()[name] = module
        installed.append(name)
    return installed


_backend = _selected_backend()
if _backend == _STUB_BACKEND:
    _INSTALLED = _install_stub()
    _USING_STUB = True
elif _backend in {"PySide6", "PySide2"}:
    _INSTALLED = _install_real(_backend)
    _USING_STUB = False
else:
    raise RuntimeError(
        f"{BACKEND_ENV}={_backend!r} is not a known PySide backend; "
        "leave it unset (pure-Python stub) or set it to PySide6/PySide2"
    )


if _USING_STUB:
    # ``QT_TRANSLATE_NOOP`` lives in QtCore. The package re-exports it because
    # `from PySide import QT_TRANSLATE_NOOP` is a legal Qt5 spelling.
    QT_TRANSLATE_NOOP = QtCore.QT_TRANSLATE_NOOP
else:
    QT_TRANSLATE_NOOP = getattr(QtCore, "QT_TRANSLATE_NOOP", _stub.QT_TRANSLATE_NOOP)

#: True when the modules above are the pure-Python stub, False for a real Qt.
USING_STUB = _USING_STUB
globals()[_stub.STUB_MARKER] = _USING_STUB

__all__ = ["QT_TRANSLATE_NOOP", *_INSTALLED]


def __getattr__(name):
    if name.startswith("__") and name.endswith("__"):
        raise AttributeError(name)
    raise AttributeError(
        f"module {__name__!r} has no attribute {name!r}; "
        f"it exports only {{{', '.join(__all__)}}}. "
        "Set FERROCAD_PYSIDE_BACKEND=PySide6 to import a real Qt."
    )
