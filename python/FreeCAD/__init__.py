"""A drop-in, Rust-backed stand-in for FreeCAD's Python ``App`` module.

The public surface mirrors the parts of upstream FreeCAD that headless scripts
touch::

    import FreeCAD
    doc = FreeCAD.newDocument("HelloWorld")
    obj = doc.addObject("App::FeaturePython", "Box")
    doc.recompute()

Two Rust backends provide the implementation; this module just selects one:

* ``ferrocad``         — the PyO3 bindings over ``ferrocad_core`` (primary, M3b).
* ``_ctypes_backend``  — the M0 C-ABI bridge, used if ``ferrocad`` is unavailable.

The C++ PyCXX bindings are not involved at all.

``fc`` is a thin binding over the document object model; it has no notion of an
"application" (document registry, active document, version). This facade adds
that small layer in Python so the ``FreeCAD`` module surface stays complete.
"""

from __future__ import annotations

try:  # pragma: no cover - trivial branch
    import ferrocad as _fc

    backend = "ferrocad"
except ImportError:  # pragma: no cover - depends on build artifacts
    _fc = None
    backend = "ctypes"
    from . import _ctypes_backend as _ctypes

if backend == "ferrocad":
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
    BoundBox = _fc.BoundBox
    Material = _fc.Material
    Vector2d = _fc.Vector2d
    GuiUp = 0
    __version__ = _fc.__version__

    # Core/utility submodules (the M4 Base/Units/Console surface).
    from . import Base, Units, Console

    ScaleType = Base.ScaleType

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
    "ScaleType",
    "writeRecoverySnapshotToTransientDir",
]


def __getattr__(name):
    # PEP 562: resolved lazily because the active document changes over time.
    if name == "ActiveDocument":
        return activeDocument()
    raise AttributeError("module 'FreeCAD' has no attribute '%s'" % name)


# ---------------------------------------------------------------------------
# Parameters (App-level config store) — in-memory shim with typed values and
# nested groups.
# ---------------------------------------------------------------------------

# root path -> {"values": {(group, name, kind): value}, "groups": {group, ...}}
_parameter_stores = {}


def _param_store(root):
    return _parameter_stores.setdefault(root, {"values": {}, "groups": set()})


def _collapse(path):
    return "/".join(part for part in path.split("/") if part)


class ParameterGrp:
    """An in-memory stand-in for ``Base.ParameterGrp`` (typed, nested groups)."""

    def __init__(self, path=""):
        path = path.strip("/")
        if ":" in path:
            root, _, rest = path.partition(":")
        else:
            root, rest = path, ""
        self._init(root, rest.strip("/"), _param_store(root))

    def _init(self, root, group, store):
        self._root = root
        self._group = group
        self._store = store
        store["groups"].add(group)

    def _full(self, sub):
        return (self._group + "/" + sub) if self._group else sub

    def _get(self, name, kind, default):
        return self._store["values"].get((self._group, name, kind), default)

    def _set(self, name, kind, value):
        self._store["values"][(self._group, name, kind)] = value

    def _rem(self, name, kind):
        self._store["values"].pop((self._group, name, kind), None)

    # -- groups -------------------------------------------------------------
    def GetGroup(self, name, _store=None, _group=None):
        sub = _collapse(name)
        if not sub:
            raise ValueError("empty group name")
        child = ParameterGrp.__new__(ParameterGrp)
        child._init(self._root, self._full(sub), self._store)
        return child

    def GetGroupName(self):
        return self._group.split("/")[-1] if self._group else ""

    def HasGroup(self, name):
        return self._full(_collapse(name)) in self._store["groups"]

    def RemGroup(self, name):
        # POC: a live reference keeps the group alive, so this is a no-op.
        pass

    def Clear(self):
        group = self._group
        for key in list(self._store["values"]):
            if group == "" or key[0] == group or key[0].startswith(group + "/"):
                del self._store["values"][key]

    # -- typed accessors ----------------------------------------------------
    def GetInt(self, name, default=0):
        return int(self._get(name, "int", default))

    def SetInt(self, name, value):
        self._set(name, "int", int(value))

    def RemInt(self, name):
        self._rem(name, "int")

    def GetBool(self, name, default=False):
        return bool(self._get(name, "bool", default))

    def SetBool(self, name, value):
        self._set(name, "bool", bool(value))

    def RemBool(self, name):
        self._rem(name, "bool")

    def GetFloat(self, name, default=0.0):
        return float(self._get(name, "float", default))

    def SetFloat(self, name, value):
        self._set(name, "float", float(value))

    def RemFloat(self, name):
        self._rem(name, "float")

    def GetString(self, name, default=""):
        return str(self._get(name, "string", default))

    def SetString(self, name, value):
        self._set(name, "string", str(value))

    def RemString(self, name):
        self._rem(name, "string")

    # -- import / export ----------------------------------------------------
    def Export(self, path):
        import json

        data = {}
        group = self._group
        for (g, name, kind), value in self._store["values"].items():
            if g == group or g.startswith(group + "/"):
                rel = g[len(group):].lstrip("/")
                data["%s\t%s\t%s" % (rel, name, kind)] = value
        with _builtins.open(path, "w", encoding="utf-8") as handle:
            json.dump(data, handle)

    def Import(self, path):
        import json

        with _builtins.open(path, "r", encoding="utf-8") as handle:
            data = json.load(handle)
        group = self._group
        for key, value in data.items():
            rel, name, kind = key.split("\t")
            target = (group + "/" + rel) if rel else group
            self._store["values"][(target, name, kind)] = value
            self._store["groups"].add(target)

    def __repr__(self):
        return "<ParameterGrp '%s:%s'>" % (self._root, self._group)


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
