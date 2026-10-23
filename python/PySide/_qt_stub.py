"""Pure-Python stand-ins for the slice of Qt the upstream Draft import path uses.

This module is imported by :mod:`PySide` when no real Qt backend is requested
(see ``python/PySide/__init__.py``). It exists so that upstream FreeCAD scripts
can be imported with **no Qt runtime installed**:

* ``draftutils/params.py`` needs ``QtCore.QFile``/``QIODevice`` over a mod's
  loose ``Resources/`` tree, plus ``QStandardPaths`` for the font fallback;
* ``draftutils/translate.py`` needs ``QCoreApplication.translate`` and
  ``QApplication.UnicodeUTF8`` (which it probes in a ``try/except``);
* ``draftobjects/*.py`` need ``QT_TRANSLATE_NOOP``;
* ``draftviewproviders/view_base.py`` needs the module objects, plus
  ``QObject``/``SIGNAL`` and a few Qt class names used as base classes.

Everything here is deliberately small. This is **not** a Qt reimplementation:
widgets are not constructed, signals are not wired to an event loop, and any
name outside an exported allowlist raises :class:`AttributeError` naming the
module and the missing name. The platform backend is a Rust/Python consumer, not
these classes.

The resource reader (:class:`QFile`) is plain Python: it accepts ``:/…`` paths,
searches every import root for ``Resources/<relative>`` (with or without one
intermediate workbench directory), and caches both hits and misses for the
import-time burst of ``params.py``.
"""

from __future__ import annotations

import os
import sys
import threading
import types

#: Marker read by the tests (and useful in a debugger) to tell a stub module
#: from an aliased real Qt module.
STUB_MARKER = "__ferrocad_pyside_stub__"


# ---------------------------------------------------------------------------
# Modules with a loud, contextual AttributeError
# ---------------------------------------------------------------------------


class QtStubModule(types.ModuleType):
    """A module whose missing names fail with the module and name spelled out."""

    def __getattr__(self, name):
        exports = self.__dict__.get("__all__", ())
        if name.startswith("__") and name.endswith("__"):
            # Protocol probes (pickle, inspect, copy, ...) keep the cheap message.
            raise AttributeError(name)
        raise AttributeError(
            f"module {self.__name__!r} has no attribute {name!r}; "
            f"the FerroCAD PySide shim exports only {{{', '.join(exports)}}}. "
            "Set FERROCAD_PYSIDE_BACKEND=PySide6 to import a real Qt."
        )


# ---------------------------------------------------------------------------
# Resources: ``:/…`` -> a mod's loose ``Resources/`` directory
# ---------------------------------------------------------------------------

#: Relative resource path -> bytes, or ``None`` when no mod ships it. Misses are
#: cached too: ``params.py`` probes 16 files at import, most of which are absent.
_RESOURCE_CACHE: dict[str, bytes | None] = {}


def clear_resource_cache() -> None:
    """Drop cached resource lookups (used by tests and after a ``sys.path`` change)."""
    _RESOURCE_CACHE.clear()


def _resource_roots():
    """Import roots worth searching, in ``sys.path`` order."""
    for entry in sys.path:
        if entry and os.path.isdir(entry):
            yield entry


def resource_bytes(relative: str) -> bytes | None:
    """Bytes for ``:/<relative>``, or ``None`` when no mod ships it.

    Two layouts are recognised, matching what the app and the conformance
    harness put on ``sys.path``:

    * ``<root>/Resources/<relative>`` — ``root`` is a workbench directory
      (``src/Mod/Draft``);
    * ``<root>/<Mod>/Resources/<relative>`` — ``root`` is a mods directory.
    """
    if relative in _RESOURCE_CACHE:
        return _RESOURCE_CACHE[relative]

    found = None
    for root in _resource_roots():
        candidate = os.path.join(root, "Resources", relative)
        if os.path.isfile(candidate):
            found = candidate
            break
    if found is None:
        for root in _resource_roots():
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


class QByteArray:
    """The slice of ``QByteArray`` that ``readAll().data().decode()`` needs."""

    __slots__ = ("_data",)

    def __init__(self, data=b"", /):
        if isinstance(data, QByteArray):
            data = data._data
        self._data = bytes(data)

    def data(self) -> bytes:
        return self._data

    def constData(self) -> bytes:
        return self._data

    def size(self) -> int:
        return len(self._data)

    def isEmpty(self) -> bool:
        return not self._data

    def toStdString(self) -> str:
        return self._data.decode("utf-8", "replace")

    def __bytes__(self) -> bytes:
        return self._data

    def __len__(self) -> int:
        return len(self._data)

    def __getitem__(self, item):
        return self._data[item]

    def __eq__(self, other):
        if isinstance(other, QByteArray):
            return self._data == other._data
        if isinstance(other, (bytes, bytearray)):
            return self._data == bytes(other)
        return NotImplemented

    def __repr__(self) -> str:
        return f"QByteArray({self._data!r})"


# ---------------------------------------------------------------------------
# QtCore
# ---------------------------------------------------------------------------


class QIODevice:
    """Open-mode flags and the read side of the device interface.

    Only reading is implemented. Writing raises rather than pretending to
    succeed, because no consumer on the headless path writes through Qt.
    """

    NotOpen = 0x0000
    ReadOnly = 0x0001
    WriteOnly = 0x0002
    ReadWrite = ReadOnly | WriteOnly
    Append = 0x0004
    Truncate = 0x0008
    Text = 0x0010
    Unbuffered = 0x0020

    def __init__(self, *args, **kwargs):
        self._open = False

    def isOpen(self) -> bool:
        return self._open

    def isReadable(self) -> bool:
        return self._open

    def isWritable(self) -> bool:
        return False

    def close(self) -> None:
        self._open = False

    def open(self, mode):  # pragma: no cover - overridden, signature parity
        raise NotImplementedError(
            "PySide shim: QIODevice.open() is abstract; use QFile"
        )


class QFile(QIODevice):
    """A read-only file over a real path **or** a ``:/`` mod resource.

    ``open()`` returns ``True``/``False`` exactly like Qt (rather than raising),
    because ``draftutils/params.py`` relies on ``False`` to skip preference
    files that the installed workbenches do not ship.
    """

    def __init__(self, path="", /, *args, **kwargs):
        super().__init__()
        self._path = os.fspath(path)
        self._data: bytes | None = None
        self._pos = 0

    # -- introspection ----------------------------------------------------
    def fileName(self) -> str:
        return self._path

    def exists(self) -> bool:
        if self._path.startswith(":/"):
            return resource_bytes(self._path[2:]) is not None
        return os.path.isfile(self._path)

    def size(self) -> int:
        if self._data is None:
            return 0
        return len(self._data)

    # -- reading -----------------------------------------------------------
    def open(self, mode) -> bool:
        if mode & (self.WriteOnly | self.Append | self.Truncate):
            raise NotImplementedError(
                "PySide shim: QFile is read-only; "
                f"open mode {mode!r} for {self._path!r} needs a real Qt"
            )
        if self._path.startswith(":/"):
            data = resource_bytes(self._path[2:])
        else:
            try:
                with open(self._path, "rb") as handle:
                    data = handle.read()
            except OSError:
                data = None
        if data is None:
            self._data = None
            self._pos = 0
            self._open = False
            return False
        self._data = data
        self._pos = 0
        self._open = True
        return True

    def _remaining(self) -> bytes:
        if not self._open or self._data is None:
            raise ValueError(
                f"PySide shim: read from a closed QFile ({self._path!r}); "
                "call open() first"
            )
        return self._data[self._pos :]

    def readAll(self) -> QByteArray:
        chunk = self._remaining()
        self._pos = len(self._data)
        return QByteArray(chunk)

    def read(self, maxSize=-1) -> QByteArray:
        chunk = self._remaining()
        if maxSize is None or maxSize < 0:
            chunk, consumed = chunk, len(chunk)
        else:
            chunk, consumed = chunk[:maxSize], min(maxSize, len(chunk))
        self._pos += consumed
        return QByteArray(chunk)

    def readLine(self, maxSize=0) -> QByteArray:
        chunk = self._remaining()
        newline = chunk.find(b"\n")
        end = len(chunk) if newline < 0 else newline + 1
        if maxSize and maxSize > 0:
            end = min(end, maxSize)
        self._pos += end
        return QByteArray(chunk[:end])

    def atEnd(self) -> bool:
        return self._data is None or self._pos >= len(self._data)


class QFileInfo:
    """Path introspection; ``:/`` paths resolve through the resource map."""

    def __init__(self, path="", /):
        self._path = os.fspath(path)

    def filePath(self) -> str:
        return self._path

    def absoluteFilePath(self) -> str:
        return os.path.abspath(self._path) if not self._path.startswith(":/") else self._path

    def fileName(self) -> str:
        return os.path.basename(self._path)

    def exists(self) -> bool:
        if self._path.startswith(":/"):
            return resource_bytes(self._path[2:]) is not None
        return os.path.exists(self._path)

    def isFile(self) -> bool:
        if self._path.startswith(":/"):
            return resource_bytes(self._path[2:]) is not None
        return os.path.isfile(self._path)


def QT_TRANSLATE_NOOP(context, text):
    """Qt5's translation marker: return the source string unchanged."""
    return text


class QCoreApplication:
    """Static translation surface only; there is no Qt event loop here."""

    @staticmethod
    def translate(context, text, disambiguation=None, n=-1):
        return text

    @classmethod
    def instance(cls):
        # Like Qt when no application object exists.
        return None

    @staticmethod
    def installTranslator(translator):
        raise NotImplementedError(
            "PySide shim: QCoreApplication.installTranslator() needs a real Qt"
        )


class QLocale:
    """The decimal separator (and nothing else) used by ``draftutils/units.py``."""

    def __init__(self, *args, **kwargs):
        self._name = "C"

    def decimalPoint(self) -> str:
        return "."

    def name(self) -> str:
        return self._name

    def toString(self, value) -> str:
        return str(value)


class SignalSpec:
    """The object ``SIGNAL("valueChanged(double)")`` returns."""

    __slots__ = ("signature",)

    def __init__(self, signature):
        self.signature = str(signature)

    @property
    def name(self) -> str:
        return self.signature.split("(", 1)[0].strip()

    def __repr__(self) -> str:
        return f"SIGNAL({self.signature!r})"


def SIGNAL(signature) -> SignalSpec:
    """Legacy Qt4 signal spelling; the argument list is part of the contract."""
    return SignalSpec(signature)


class Signal:
    """A minimal signal: connect/disconnect/emit, no event loop, no payload cast."""

    __slots__ = ("name", "owner", "_slots", "_blocked")

    def __init__(self, name, owner=None):
        self.name = name
        self.owner = owner
        self._slots = []
        self._blocked = False

    def connect(self, slot):
        if slot not in self._slots:
            self._slots.append(slot)
        return slot

    def disconnect(self, slot=None):
        if slot is None:
            self._slots.clear()
        elif slot in self._slots:
            self._slots.remove(slot)

    def emit(self, *args):
        if self._blocked:
            return
        if self.owner is not None and getattr(self.owner, "_signals_blocked", False):
            return
        for slot in list(self._slots):
            slot(*args)

    __call__ = emit

    def __repr__(self) -> str:
        return f"<Signal {self.name!r} of {type(self.owner).__name__}>"


class QObject:
    """A parent/child holder with the legacy ``connect`` helper.

    ``QWidget`` subclasses are a non-goal for the shim, but their *class
    statements* execute at import time, so the base must be subclassable. Any
    actual signal wiring still goes through :class:`Signal`.
    """

    def __init__(self, parent=None, /):
        self._parent = None
        self._children = []
        self._object_name = ""
        self._signals_blocked = False
        self._signals: dict[str, Signal] = {}
        if parent is not None:
            self.setParent(parent)

    # -- ownership --------------------------------------------------------
    def setParent(self, parent):
        if self._parent is not None:
            try:
                self._parent._children.remove(self)
            except ValueError:
                pass
        self._parent = parent
        if parent is not None:
            parent._children.append(self)

    def parent(self):
        return self._parent

    def children(self):
        return list(self._children)

    def objectName(self) -> str:
        return self._object_name

    def setObjectName(self, name) -> None:
        self._object_name = str(name)

    def deleteLater(self) -> None:
        # There is no deferred-delete queue without an event loop.
        self.setParent(None)

    # -- signals ----------------------------------------------------------
    def blockSignals(self, block):
        previous = self._signals_blocked
        self._signals_blocked = bool(block)
        return previous

    def signalsBlocked(self) -> bool:
        return self._signals_blocked

    def signal(self, name) -> Signal:
        """Return (creating on demand) a named signal on this object."""
        signal = self._signals.get(name)
        if signal is None:
            signal = self._signals[name] = Signal(name, self)
        return signal

    @staticmethod
    def connect(receiver, signal, slot, *args):
        """Qt4's ``QObject.connect(receiver, SIGNAL("x()"), slot)``."""
        name = signal.name if isinstance(signal, SignalSpec) else str(signal)
        target = getattr(receiver, name, None)
        if not isinstance(target, Signal):
            raise AttributeError(
                "PySide shim: QObject.connect() found no signal "
                f"{name!r} on {type(receiver).__name__}; "
                "the shim only supports signals it defines"
            )
        return target.connect(slot)


class QTimer(QObject):
    """Deferred callbacks without a Qt event loop.

    ``singleShot(0, cb)`` runs ``cb`` immediately: with no event loop there is
    nothing to defer to, and upstream's ``QtCore.QTimer.singleShot(0, …)`` calls
    are "do this after the current call stack" only inside a running GUI.
    Positive delays use a daemon :class:`threading.Timer`.
    """

    def __init__(self, parent=None, /):
        super().__init__(parent)
        self.timeout = self.signal("timeout")
        self._interval = 0
        self._single_shot = False
        self._timer: threading.Timer | None = None

    def setSingleShot(self, value) -> None:
        self._single_shot = bool(value)

    def isSingleShot(self) -> bool:
        return self._single_shot

    def setInterval(self, msec) -> None:
        self._interval = int(msec)

    def interval(self) -> int:
        return self._interval

    def isActive(self) -> bool:
        return self._timer is not None and self._timer.is_alive()

    def start(self, msec=None) -> None:
        if msec is not None:
            self._interval = int(msec)
        self.stop()

        def fire():
            self._timer = None
            self.timeout.emit()
            # A zero-interval repeating timer has no event loop to yield to;
            # re-arming it would recurse forever.
            if not self._single_shot and self._interval > 0:
                self.start(self._interval)

        if self._interval <= 0:
            fire()
        else:
            self._timer = threading.Timer(self._interval / 1000.0, fire)
            self._timer.daemon = True
            self._timer.start()

    def stop(self) -> None:
        if self._timer is not None:
            self._timer.cancel()
            self._timer = None

    @staticmethod
    def singleShot(msec, receiver, member=None):
        delay = int(msec)

        def fire():
            if member is None:
                receiver()
            elif callable(member):
                member()
            elif isinstance(member, str):
                getattr(receiver, member)()
            else:  # pragma: no cover - defensive
                raise TypeError(
                    "PySide shim: QTimer.singleShot(ms, receiver, member) needs "
                    "a callable, a slot name or a callable member"
                )

        if delay <= 0:
            fire()
            return None
        timer = threading.Timer(delay / 1000.0, fire)
        timer.daemon = True
        timer.start()
        return timer


class QStandardPaths:
    """Location queries used by ``params._get_shape_string_font_file``."""

    DesktopLocation = 0
    DocumentsLocation = 1
    FontsLocation = 2
    ApplicationsLocation = 3
    MusicLocation = 4
    MoviesLocation = 5
    PicturesLocation = 6
    TempLocation = 7
    HomeLocation = 8
    DataLocation = 9
    CacheLocation = 10

    @staticmethod
    def standardLocations(location):
        import tempfile

        home = os.path.expanduser("~")
        if location == QStandardPaths.FontsLocation:
            data_home = os.environ.get("XDG_DATA_HOME") or os.path.join(
                home, ".local", "share"
            )
            data_dirs = os.environ.get("XDG_DATA_DIRS") or "/usr/local/share:/usr/share"
            candidates = [
                os.path.join(data_home, "fonts"),
                os.path.join(home, ".fonts"),
            ]
            candidates += [
                os.path.join(entry, "fonts")
                for entry in data_dirs.split(":") if entry
            ]
            seen = set()
            found = []
            for path in candidates:
                path = os.path.normpath(path)
                if path not in seen and os.path.isdir(path):
                    seen.add(path)
                    found.append(path)
            return found
        if location == QStandardPaths.HomeLocation:
            return [home] if os.path.isdir(home) else []
        if location == QStandardPaths.TempLocation:
            return [tempfile.gettempdir()]
        return []

    @staticmethod
    def writableLocation(location):
        locations = QStandardPaths.standardLocations(location)
        if not locations:
            raise NotImplementedError(
                "PySide shim: QStandardPaths.writableLocation() for "
                f"location {location!r} is not implemented"
            )
        return locations[0]


# ---------------------------------------------------------------------------
# QtWidgets
# ---------------------------------------------------------------------------


class QApplication(QCoreApplication):
    """Only ``translate`` is real; widgets themselves are out of scope."""

    @staticmethod
    def translate(context, text, disambiguation=None, n=-1):
        return text

    @classmethod
    def instance(cls):
        return None

    @staticmethod
    def processEvents(*args, **kwargs):
        raise NotImplementedError(
            "PySide shim: QApplication.processEvents() needs a real Qt event loop"
        )


class _UnsupportedWidget:
    """Base for names Draft subclasses at import time.

    Class creation succeeds (``class DraftBaseWidget(QWidget)`` must import);
    instantiation fails with the class name and the reason.
    """

    _qt_base = "QWidget"

    def __init__(self, *args, **kwargs):
        raise NotImplementedError(
            f"PySide shim: {type(self).__name__} extends {self._qt_base}, and the "
            "shim does not construct Qt widgets; render the form with ferrocad_ui "
            "or run with FERROCAD_PYSIDE_BACKEND=PySide6"
        )


class QWidget(_UnsupportedWidget):
    _qt_base = "QWidget"


class QLineEdit(_UnsupportedWidget):
    _qt_base = "QLineEdit"


class QStyledItemDelegate(_UnsupportedWidget):
    _qt_base = "QStyledItemDelegate"


# ---------------------------------------------------------------------------
# Public allowlists (``python/PySide/__init__.py`` builds the modules from these)
# ---------------------------------------------------------------------------

QTCORE_EXPORTS = {
    "QT_TRANSLATE_NOOP": QT_TRANSLATE_NOOP,
    "QCoreApplication": QCoreApplication,
    "QLocale": QLocale,
    "QObject": QObject,
    "QTimer": QTimer,
    "SIGNAL": SIGNAL,
    "QIODevice": QIODevice,
    "QFile": QFile,
    "QFileInfo": QFileInfo,
    "QStandardPaths": QStandardPaths,
}

QTGUI_EXPORTS: dict[str, object] = {}

QTWIDGETS_EXPORTS = {
    "QApplication": QApplication,
    "QWidget": QWidget,
    "QLineEdit": QLineEdit,
    "QStyledItemDelegate": QStyledItemDelegate,
}

__all__ = [
    "QTCORE_EXPORTS",
    "QTGUI_EXPORTS",
    "QTWIDGETS_EXPORTS",
    "QtStubModule",
    "QByteArray",
    "QIODevice",
    "QFile",
    "QFileInfo",
    "QObject",
    "QTimer",
    "Signal",
    "SignalSpec",
    "SIGNAL",
    "QCoreApplication",
    "QApplication",
    "QLocale",
    "QStandardPaths",
    "QT_TRANSLATE_NOOP",
    "clear_resource_cache",
    "resource_bytes",
]
