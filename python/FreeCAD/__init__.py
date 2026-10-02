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
    Vector = _fc.Vector
    Matrix = _fc.Matrix
    Placement = _fc.Placement
    Rotation = _fc.Rotation
    TypeId = _fc.TypeId
    GuiUp = 0
    __version__ = _fc.__version__

    # Core/utility submodules (the M4 Base/Units/Console surface).
    from . import Base, Units, Console

    addDocumentObserver = _fc.addDocumentObserver
    removeDocumentObserver = _fc.removeDocumentObserver

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

    def open(name, hidden=False, temporary=False):
        global _active
        doc = _fc.openDocument(name)
        _documents[doc.Name] = doc
        _active = doc
        return doc

    # Upstream alias (`FreeCAD.openDocument(path)`).
    def openDocument(path, hidden=False, temporary=False):
        return open(path, hidden=hidden, temporary=temporary)

    def closeDocument(name):
        global _active
        if name not in _documents:
            raise ValueError("no document named '%s'" % name)
        doc = _documents.pop(name)
        if _active is not None and _active.Name == name:
            _active = None
        _fc._emitDocument("slotDeletedDocument", doc)
        _fc._forgetDocument(doc)

    def getDocument(name):
        return _documents.get(name)

    def setActiveDocument(name):
        global _active
        if name in _documents:
            doc = _documents[name]
            if doc is not _active:
                _active = doc
                _fc._emitDocument("slotActivateDocument", doc)

    def listDocuments():
        return dict(_documents)

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
    "openDocument",
    "activeDocument",
    "Document",
    "DocumentObject",
    "ActiveDocument",
    "backend",
    "ParamGet",
    "ParameterGrp",
    "writeRecoverySnapshotToTransientDir",
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


# ---------------------------------------------------------------------------
# Recovery snapshots (App-level; writes metadata + document to the transient
# directory of a document).
# ---------------------------------------------------------------------------

import builtins as _builtins  # the facade defines `open` (document); keep the builtin.


def _xml_escape(text):
    return (
        str(text)
        .replace("&", "&amp;")
        .replace("<", "&lt;")
        .replace(">", "&gt;")
        .replace('"', "&quot;")
        .replace("'", "&apos;")
    )


def _document_xml(doc):
    """A minimal ``Document.xml`` body for a recovery snapshot."""
    parts = ['<?xml version="1.0" encoding="utf-8"?>', "<Document>"]
    for obj in doc.Objects:
        parts.append(
            '  <Object name="%s" type="%s" />' % (_xml_escape(obj.Name), _xml_escape(obj.TypeId))
        )
    parts.append("</Document>")
    return "\n".join(parts) + "\n"


def writeRecoverySnapshotToTransientDir(doc, compressed=True):
    """Write a recovery snapshot (metadata + document) to ``doc.TransientDir``.

    Raises ``RuntimeError`` while a transaction is open (see
    ``Document.canWriteRecoverySnapshot``).
    """
    if not doc.canWriteRecoverySnapshot():
        raise RuntimeError("cannot write a recovery snapshot while a transaction is open")

    import os
    import zipfile

    transient = doc.TransientDir
    os.makedirs(transient, exist_ok=True)

    metadata = os.path.join(transient, "fc_recovery_file.xml")
    with _builtins.open(metadata, "w", encoding="utf-8") as handle:
        handle.write('<?xml version="1.0" encoding="utf-8"?>\n')
        handle.write("<AutoRecovery>\n")
        handle.write("  <Label>%s</Label>\n" % _xml_escape(doc.Label))
        handle.write("  <FileName>%s</FileName>\n" % _xml_escape(doc.FileName))
        handle.write("</AutoRecovery>\n")

    document_xml = _document_xml(doc)
    if compressed:
        archive = os.path.join(transient, "fc_recovery_file.fcstd")
        with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as recovery:
            recovery.writestr("Document.xml", document_xml)
    else:
        uncompressed = os.path.join(transient, "fc_recovery_files")
        os.makedirs(uncompressed, exist_ok=True)
        with _builtins.open(os.path.join(uncompressed, "Document.xml"), "w", encoding="utf-8") as handle:
            handle.write(document_xml)

    return True


class PropertyType:
    """Property type bit flags (``FreeCAD.PropertyType.*``)."""

    Prop_None = 0
    Prop_ReadOnly = 1
    Prop_Transient = 2
    Prop_Hidden = 4
    Prop_Output = 8
    Prop_NoRecompute = 16
    Prop_NoPersist = 32
