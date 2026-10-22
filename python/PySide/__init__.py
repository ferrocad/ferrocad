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

**Resources.** A Qt resource path (``:/ui/preferences-draft.ui``) normally needs
a compiled ``.rcc``. FerroCAD ships mods as loose files, so ``QtCore.QFile`` is
replaced (in place, preserving module identity) with a subclass that serves
``:/`` paths from a mod's ``Resources/`` directory and delegates every other
path to Qt. That is what lets upstream workbenches parse their ``.ui``
defaults without pyside-rcc; it is a compatibility shim, not a Qt reimplementation.
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


# -- resource paths -------------------------------------------------------
#: Absolute path -> bytes, or None when no mod ships the resource. Mod trees are
#: small and `_get_param_dictionary` reads ~16 files at import, so a cache is
#: enough; a miss is cached too.
_RESOURCE_CACHE = {}


def _resource_bytes(relative):
    """Bytes for a ``:/<relative>`` resource, from a mod's ``Resources/`` dir."""
    import os

    if relative in _RESOURCE_CACHE:
        return _RESOURCE_CACHE[relative]

    found = None
    roots = [entry for entry in sys.path if entry and os.path.isdir(entry)]
    # `<root>/Resources/<rel>` is a mod directory on the path (the conformance
    # harness adds `src/Mod/Draft`); `<root>/<Mod>/Resources/<rel>` is a mods
    # directory (the app adds `mods/`).
    for root in roots:
        candidate = os.path.join(root, "Resources", relative)
        if os.path.isfile(candidate):
            found = candidate
            break
    if found is None:
        for root in roots:
            try:
                children = sorted(os.listdir(root))
            except OSError:
                continue
            for child in children:
                candidate = os.path.join(root, child, "Resources", relative)
                if os.path.isfile(candidate):
                    found = candidate
                    break
            if found is not None:
                break

    data = None
    if found is not None:
        with open(found, "rb") as handle:
            data = handle.read()
    _RESOURCE_CACHE[relative] = data
    return data


class _ReadAll:
    """The tiny slice of ``QByteArray`` that ``readAll().data().decode()`` uses."""

    def __init__(self, data):
        self._data = data

    def data(self):
        return self._data

    def __len__(self):
        return len(self._data)


def _install_resource_qfile(qtcore):
    """Let ``QFile(':/…')`` read a mod's loose ``Resources/`` files."""
    base = qtcore.QFile

    class ResourceQFile(base):
        """``QFile`` that serves ``:/`` paths from the mod tree."""

        def __init__(self, path=""):
            super().__init__(path)
            self._resource = None
            text = str(path)
            if text.startswith(":/"):
                self._resource = _resource_bytes(text[2:])

        def open(self, mode):
            if self._resource is not None:
                return True
            return super().open(mode)

        def readAll(self):
            if self._resource is not None:
                return _ReadAll(self._resource)
            return super().readAll()

        def close(self):
            if self._resource is not None:
                return
            super().close()

    # In place, so `PySide.QtCore is PySide6.QtCore` stays true. Only `:/`
    # paths change behaviour; every other path reaches Qt untouched.
    qtcore.QFile = ResourceQFile


_install_aliases()

_qtcore_module = sys.modules.get(f"{__name__}.QtCore")
if _qtcore_module is not None:
    _install_resource_qfile(_qtcore_module)


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
