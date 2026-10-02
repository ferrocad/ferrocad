"""A drop-in, Rust-backed stand-in for FreeCAD's Python ``App`` module.

The public surface mirrors the parts of upstream FreeCAD that headless scripts
touch::

    import FreeCAD
    doc = FreeCAD.newDocument("HelloWorld")
    obj = doc.addObject("App::FeaturePython", "Box")
    doc.recompute()

Two Rust backends provide the implementation; this module just selects one:

* ``fc``               — the PyO3 bindings over ``fc-core`` (primary, M3b).
* ``_ctypes_backend``  — the M0 C-ABI bridge, used if ``fc`` is unavailable.

The C++ PyCXX bindings are not involved at all.

``fc`` is a thin binding over the document object model; it has no notion of an
"application" (document registry, active document, version). This facade adds
that small layer in Python so the ``FreeCAD`` module surface stays complete.
"""

from __future__ import annotations

try:  # pragma: no cover - trivial branch
    import fc as _fc

    backend = "fc"
except ImportError:  # pragma: no cover - depends on build artifacts
    _fc = None
    backend = "ctypes"
    from . import _ctypes_backend as _ctypes

if backend == "fc":
    Document = _fc.Document
    DocumentObject = _fc.DocumentObject
    Quantity = _fc.Quantity
    StringHasher = _fc.StringHasher
    StringID = _fc.StringID
    __version__ = _fc.__version__

    # Core/utility submodules (the M4 Base/Units/Console surface).
    from . import Base, Units, Console

    # `fc` has no App/registry, so the facade owns it: a name -> Document map,
    # an "active" pointer, and FreeCAD-style unique name allocation.
    _documents = {}
    _active = None

    def _unique_name(name):
        base = name or "Unnamed"
        candidate = base
        i = 1
        while candidate in _documents:
            candidate = "%s%03d" % (base, i)
            i += 1
        return candidate

    def newDocument(name=None, hidden=False, temp=False):
        global _active
        doc_name = _unique_name(name)
        doc = _fc.newDocument(doc_name)
        _documents[doc_name] = doc
        _active = doc
        return doc

    def closeDocument(name):
        global _active
        if name not in _documents:
            raise ValueError("no document named '%s'" % name)
        del _documents[name]
        if _active is not None and _active.Name == name:
            _active = None

    def getDocument(name):
        return _documents.get(name)

    def listDocuments():
        return list(_documents)

    def activeDocument():
        return _active

    def Version():
        return __version__.split(".") + ["rust-fc", ""]

else:
    Document = _ctypes.Document
    DocumentObject = _ctypes.DocumentObject
    newDocument = _ctypes.newDocument
    closeDocument = _ctypes.closeDocument
    getDocument = _ctypes.getDocument
    listDocuments = _ctypes.listDocuments
    activeDocument = _ctypes.activeDocument
    Version = _ctypes.Version
    __version__ = _ctypes.__version__

__all__ = [
    "Version",
    "newDocument",
    "closeDocument",
    "getDocument",
    "listDocuments",
    "activeDocument",
    "Document",
    "DocumentObject",
    "ActiveDocument",
    "backend",
    "ParamGet",
    "ParameterGrp",
]


def __getattr__(name):
    # PEP 562: resolved lazily because the active document changes over time.
    if name == "ActiveDocument":
        return activeDocument()
    raise AttributeError("module 'FreeCAD' has no attribute '%s'" % name)


# ---------------------------------------------------------------------------
# Parameters (App-level config store) — minimal in-memory shim
# ---------------------------------------------------------------------------


class ParameterGrp:
    """A minimal, in-memory stand-in for ``Base.ParameterGrp``."""

    def __init__(self, path=""):
        self._path = path
        self._values = {}

    def GetInt(self, name, default=0):
        return int(self._values.get(name, default))

    def GetBool(self, name, default=False):
        return bool(self._values.get(name, default))

    def GetFloat(self, name, default=0.0):
        return float(self._values.get(name, default))

    def GetString(self, name, default=""):
        return str(self._values.get(name, default))

    def SetInt(self, name, value):
        self._values[name] = int(value)

    def SetBool(self, name, value):
        self._values[name] = bool(value)

    def SetFloat(self, name, value):
        self._values[name] = float(value)

    def SetString(self, name, value):
        self._values[name] = str(value)

    def GetGroup(self, name):
        return ParameterGrp(self._path + "/" + name)

    def __repr__(self):
        return "<ParameterGrp '%s'>" % self._path


def ParamGet(path="", create=True):
    """Return the parameter group rooted at ``path`` (in-memory)."""
    return ParameterGrp(path)
